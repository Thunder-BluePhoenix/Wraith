package transport

import (
	"fmt"
	"io"
	"log"
	"net"
	"time"

	"github.com/wraith/transfer/pkg/delta"
)

// Receiver listens for incoming snapshot transfers from a Transmitter.
type Receiver struct {
	listener net.Listener
	timeout  time.Duration
}

// NewReceiver starts a TCP listener on addr (:port or host:port).
// Use ":0" to let the OS pick a free port (useful in tests).
func NewReceiver(addr string, timeout time.Duration) (*Receiver, error) {
	ln, err := net.Listen("tcp", addr)
	if err != nil {
		return nil, fmt.Errorf("listen on %s: %w", addr, err)
	}
	log.Printf("[rx] listening on %s", ln.Addr())
	return &Receiver{listener: ln, timeout: timeout}, nil
}

// Addr returns the listener's local address (useful when port was :0).
func (r *Receiver) Addr() net.Addr { return r.listener.Addr() }

// Close shuts down the listener. Blocks waiting for an active AcceptSnapshot
// to finish are interrupted.
func (r *Receiver) Close() error { return r.listener.Close() }

// AcceptSnapshot blocks until a Transmitter connects, receives the full
// snapshot, and returns the raw Protobuf bytes plus the sender's Handshake.
//
// Every received DataBlock is checksum-verified before being written into
// the output buffer. If any block fails verification the transfer is aborted
// and the sender is notified.
//
// On success, write the returned bytes to disk and hand them to wraith-restorer.
func (r *Receiver) AcceptSnapshot() ([]byte, *Handshake, error) {
	conn, err := r.listener.Accept()
	if err != nil {
		return nil, nil, fmt.Errorf("accept: %w", err)
	}
	log.Printf("[rx] connection from %s", conn.RemoteAddr())

	c := &Conn{Conn: conn, Timeout: r.timeout}
	defer c.Close()

	return receiveOne(c)
}

// receiveOne handles a single transfer session on connection c.
func receiveOne(c *Conn) ([]byte, *Handshake, error) {
	// ── 1. Handshake ────────────────────────────────────────────────────────
	frame, err := c.ReadFrame()
	if err != nil {
		return nil, nil, fmt.Errorf("read handshake frame: %w", err)
	}
	if frame.Type != MsgHandshake {
		return nil, nil, fmt.Errorf("expected MsgHandshake (0x01), got 0x%02x", frame.Type)
	}
	var hs Handshake
	if err := decodeJSON(frame.Payload, &hs); err != nil {
		return nil, nil, fmt.Errorf("decode handshake: %w", err)
	}
	log.Printf("[rx] handshake: pid=%d arch=%s %d bytes %d pages v%s",
		hs.SourcePid, hs.Arch, hs.TotalBytes, hs.PageCount, hs.SnapshotVersion)

	// Pre-flight: reject incompatible or empty transfers immediately.
	if hs.Arch != "x86_64" {
		_ = c.WriteJSON(MsgReadyAck, ReadyAck{Accepted: false, Reason: fmt.Sprintf("unsupported arch: %s", hs.Arch)})
		return nil, &hs, fmt.Errorf("incompatible architecture: %s", hs.Arch)
	}
	if hs.TotalBytes == 0 {
		_ = c.WriteJSON(MsgReadyAck, ReadyAck{Accepted: false, Reason: "empty snapshot"})
		return nil, &hs, fmt.Errorf("sender reported zero-byte snapshot")
	}

	// ── 2. ReadyAck ─────────────────────────────────────────────────────────
	if err := c.WriteJSON(MsgReadyAck, ReadyAck{Accepted: true}); err != nil {
		return nil, &hs, fmt.Errorf("send ready ack: %w", err)
	}
	log.Printf("[rx] sent ready ack, waiting for data")

	// ── 3. Receive DataBlocks ────────────────────────────────────────────────
	// Pre-allocate the full snapshot buffer. TotalBytes comes from the sender's
	// Handshake; we cap it at a sanity limit to avoid OOM on malformed input.
	const maxSnapshotBytes = 128 << 30 // 128 GB
	if hs.TotalBytes > maxSnapshotBytes {
		return nil, &hs, fmt.Errorf("snapshot size %d exceeds limit %d", hs.TotalBytes, maxSnapshotBytes)
	}

	buf := make([]byte, hs.TotalBytes)
	var bytesWritten uint64
	var blocksReceived int

	for {
		frame, err := c.ReadFrame()
		if err == io.EOF {
			break
		}
		if err != nil {
			return nil, &hs, fmt.Errorf("read data frame: %w", err)
		}

		switch frame.Type {

		case MsgDataBlock:
			block, err := ReadBlock(frame.Payload)
			if err != nil {
				return nil, &hs, fmt.Errorf("decode block: %w", err)
			}

			end := block.Offset + uint64(len(block.Data))
			if end > hs.TotalBytes {
				return nil, &hs, fmt.Errorf(
					"block at offset %d+%d overflows snapshot (%d bytes)",
					block.Offset, len(block.Data), hs.TotalBytes,
				)
			}

			// Verify checksum before writing to catch corruption in transit.
			computed := delta.HashPage(block.Data)
			if computed != block.Checksum {
				return nil, &hs, fmt.Errorf(
					"checksum mismatch at offset %d: expected %016x, got %016x",
					block.Offset, block.Checksum, computed,
				)
			}

			copy(buf[block.Offset:end], block.Data)
			bytesWritten += uint64(len(block.Data))
			blocksReceived++

		case MsgComplete:
			log.Printf("[rx] complete signal: %d blocks, %d bytes", blocksReceived, bytesWritten)
			goto done

		case MsgError:
			var errMsg ErrorMsg
			_ = decodeJSON(frame.Payload, &errMsg)
			return nil, &hs, fmt.Errorf("sender reported error: %s", errMsg.Message)

		default:
			log.Printf("[rx] unknown frame type 0x%02x, skipping", frame.Type)
		}
	}

done:
	log.Printf("[rx] snapshot received: %d MB in %d blocks",
		bytesWritten/1024/1024, blocksReceived)

	return buf[:bytesWritten], &hs, nil
}
