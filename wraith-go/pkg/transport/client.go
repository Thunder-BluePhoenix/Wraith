package transport

import (
	"fmt"
	"log"
	"net"
	"time"

	"github.com/wraith/transfer/pkg/delta"
)

// TransferOptions configures a snapshot send operation.
type TransferOptions struct {
	Hostname        string
	Pid             uint32
	SnapshotVersion string
	Timeout         time.Duration
	MaxRetries      int // per-block retries on transient write errors
}

// TransferStats reports the outcome of a SendSnapshot call.
type TransferStats struct {
	TotalPages int
	TotalBytes uint64
	PagesSent  int
	BytesSent  uint64
	// Reduction is the fraction of bytes NOT sent (0.0 = nothing saved, 1.0 = nothing sent).
	// For first-time transfers this is always 0. Rises towards 1.0 for live-migration
	// rounds (Phase 8.5) where most pages haven't changed.
	Reduction float64
}

// Transmitter sends a ProcessSnapshot to a remote Receiver.
type Transmitter struct {
	conn *Conn
}

// NewTransmitter dials addr (host:port) and returns a ready Transmitter.
// timeout applies to each individual read or write operation.
func NewTransmitter(addr string, timeout time.Duration) (*Transmitter, error) {
	dialTimeout := timeout
	if dialTimeout <= 0 {
		dialTimeout = 30 * time.Second
	}
	c, err := net.DialTimeout("tcp", addr, dialTimeout)
	if err != nil {
		return nil, fmt.Errorf("dial %s: %w", addr, err)
	}
	return &Transmitter{conn: &Conn{Conn: c, Timeout: timeout}}, nil
}

// SendSnapshot transmits the snapshot bytes to the connected Receiver.
//
// Protocol flow:
//
//  1. Send Handshake (JSON)
//  2. Wait for ReadyAck  (JSON)
//  3. Send dirty DataBlocks (binary) — first call = all pages
//  4. Send MsgComplete (empty)
//
// Returns stats and any error. On error the source process can be unfrozen
// because no data has been committed on the destination yet.
func (t *Transmitter) SendSnapshot(data []byte, opts TransferOptions) (*TransferStats, error) {
	pageCount := uint32((len(data) + PageSize - 1) / PageSize)
	stats := &TransferStats{
		TotalPages: int(pageCount),
		TotalBytes: uint64(len(data)),
	}

	// ── 1. Handshake ────────────────────────────────────────────────────────
	hs := Handshake{
		SourceHostname:  opts.Hostname,
		SourcePid:       opts.Pid,
		Arch:            "x86_64",
		TotalBytes:      uint64(len(data)),
		PageCount:       pageCount,
		SnapshotVersion: opts.SnapshotVersion,
	}
	if err := t.conn.WriteJSON(MsgHandshake, hs); err != nil {
		return stats, fmt.Errorf("send handshake: %w", err)
	}
	log.Printf("[tx] handshake sent: pid=%d, %d bytes, %d pages", opts.Pid, len(data), pageCount)

	// ── 2. ReadyAck ─────────────────────────────────────────────────────────
	frame, err := t.conn.ReadFrame()
	if err != nil {
		return stats, fmt.Errorf("read ready ack: %w", err)
	}
	var ack ReadyAck
	if err := decodeJSON(frame.Payload, &ack); err != nil {
		return stats, fmt.Errorf("decode ready ack: %w", err)
	}
	if !ack.Accepted {
		return stats, fmt.Errorf("receiver rejected transfer: %s", ack.Reason)
	}
	log.Printf("[tx] receiver ready, starting transfer")

	// ── 3. Send dirty pages ──────────────────────────────────────────────────
	// DirtyOffsets returns all pages on first call (no prior manifest).
	// On subsequent calls (Phase 8.5 live migration) it returns only changed pages.
	det := delta.NewDetector()
	dirtyOffsets := det.DirtyOffsets(data, PageSize)

	for _, offset := range dirtyOffsets {
		end := offset + PageSize
		if end > len(data) {
			end = len(data)
		}
		page := data[offset:end]

		block := DataBlock{
			Offset:   uint64(offset),
			Checksum: delta.HashPage(page),
			Data:     page,
		}

		if err := t.sendBlockWithRetry(block, opts.MaxRetries); err != nil {
			return stats, fmt.Errorf("send block at offset %d: %w", offset, err)
		}

		stats.PagesSent++
		stats.BytesSent += uint64(len(page))
	}

	// ── 4. Complete ──────────────────────────────────────────────────────────
	if err := (&Frame{Type: MsgComplete}).WriteTo(t.conn.Conn); err != nil {
		return stats, fmt.Errorf("send complete: %w", err)
	}

	if stats.TotalBytes > 0 {
		stats.Reduction = 1.0 - float64(stats.BytesSent)/float64(stats.TotalBytes)
	}
	log.Printf("[tx] complete: %d/%d pages, %d MB, %.0f%% reduction",
		stats.PagesSent, stats.TotalPages,
		stats.BytesSent/1024/1024,
		stats.Reduction*100,
	)

	return stats, nil
}

// sendBlockWithRetry writes a DataBlock with exponential backoff on failure.
func (t *Transmitter) sendBlockWithRetry(block DataBlock, maxRetries int) error {
	var lastErr error
	for attempt := 0; attempt <= maxRetries; attempt++ {
		if err := t.conn.WriteBlock(block); err != nil {
			lastErr = err
			if attempt < maxRetries {
				wait := time.Duration(100*(1<<attempt)) * time.Millisecond
				log.Printf("[tx] block write error (attempt %d/%d), retrying in %s: %v",
					attempt+1, maxRetries+1, wait, err)
				time.Sleep(wait)
				continue
			}
		} else {
			return nil
		}
	}
	return lastErr
}

// Close closes the underlying TCP connection.
func (t *Transmitter) Close() error { return t.conn.Conn.Close() }
