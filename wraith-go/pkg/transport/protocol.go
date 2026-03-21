// Package transport implements the Wraith wire protocol for snapshot transfer.
//
// Wire format (all integers big-endian):
//
//	┌──────────┬───────────────┬───────────────────────────┐
//	│ 4 bytes  │   4 bytes     │   payload_len bytes       │
//	│ msg_type │  payload_len  │       payload             │
//	└──────────┴───────────────┴───────────────────────────┘
//
// Control messages (Handshake, ReadyAck, RestoreAck, ErrorMsg) use JSON payloads.
// Data messages (DataBlock) use a compact binary layout — see DataBlock below.
package transport

import (
	"encoding/binary"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"time"
)

// Message type identifiers.
const (
	MsgHandshake  = uint32(0x01) // sender → receiver: transfer metadata (JSON)
	MsgReadyAck   = uint32(0x02) // receiver → sender: accept/reject (JSON)
	MsgDataBlock  = uint32(0x03) // sender → receiver: one page of snapshot data (binary)
	MsgComplete   = uint32(0x04) // sender → receiver: all pages sent (empty payload)
	MsgRestoreAck = uint32(0x05) // receiver → sender: snapshot written to disk (JSON)
	MsgError      = uint32(0xFF) // either direction: error description (JSON)
)

// PageSize is the granularity of delta detection: 4 KB pages.
const PageSize = 4096

// ── Frame ─────────────────────────────────────────────────────────────────────

// Frame is one transport message.
type Frame struct {
	Type    uint32
	Payload []byte
}

// WriteTo encodes and writes a Frame to w.
func (f *Frame) WriteTo(w io.Writer) error {
	if err := binary.Write(w, binary.BigEndian, f.Type); err != nil {
		return fmt.Errorf("write frame type: %w", err)
	}
	if err := binary.Write(w, binary.BigEndian, uint32(len(f.Payload))); err != nil {
		return fmt.Errorf("write frame length: %w", err)
	}
	if len(f.Payload) > 0 {
		if _, err := w.Write(f.Payload); err != nil {
			return fmt.Errorf("write frame payload: %w", err)
		}
	}
	return nil
}

// ReadFrom reads one Frame from r.
func ReadFrom(r io.Reader) (*Frame, error) {
	var typ, length uint32
	if err := binary.Read(r, binary.BigEndian, &typ); err != nil {
		return nil, fmt.Errorf("read frame type: %w", err)
	}
	if err := binary.Read(r, binary.BigEndian, &length); err != nil {
		return nil, fmt.Errorf("read frame length: %w", err)
	}
	payload := make([]byte, length)
	if length > 0 {
		if _, err := io.ReadFull(r, payload); err != nil {
			return nil, fmt.Errorf("read frame payload (%d bytes): %w", length, err)
		}
	}
	return &Frame{Type: typ, Payload: payload}, nil
}

// ── Control messages (JSON) ────────────────────────────────────────────────────

// Handshake is sent by the transmitter before any data.
type Handshake struct {
	SourceHostname  string `json:"source_hostname"`
	SourcePid       uint32 `json:"source_pid"`
	Arch            string `json:"arch"`
	TotalBytes      uint64 `json:"total_bytes"`
	PageCount       uint32 `json:"page_count"`
	SnapshotVersion string `json:"snapshot_version"`
}

// ReadyAck is sent by the receiver to accept or reject the incoming transfer.
type ReadyAck struct {
	Accepted bool   `json:"accepted"`
	Reason   string `json:"reason,omitempty"`
}

// RestoreAck is sent by the receiver after the snapshot has been written to disk.
type RestoreAck struct {
	Success bool   `json:"success"`
	Error   string `json:"error,omitempty"`
}

// ErrorMsg carries a human-readable error from either side.
type ErrorMsg struct {
	Message string `json:"message"`
}

// ── DataBlock (binary) ────────────────────────────────────────────────────────

// DataBlock is one page of snapshot data on the wire.
//
// Binary layout within a MsgDataBlock frame payload:
//
//	bytes  0– 7 : offset   (uint64, BE) — byte offset in the full snapshot
//	bytes  8–15 : checksum (uint64, BE) — xxHash-64 of data
//	bytes 16–19 : data_len (uint32, BE) — number of data bytes that follow
//	bytes 20–N  : data
type DataBlock struct {
	Offset   uint64
	Checksum uint64
	Data     []byte
}

// WriteBlock encodes block into a MsgDataBlock frame and writes it to w.
func WriteBlock(w io.Writer, block DataBlock) error {
	header := make([]byte, 20)
	binary.BigEndian.PutUint64(header[0:], block.Offset)
	binary.BigEndian.PutUint64(header[8:], block.Checksum)
	binary.BigEndian.PutUint32(header[16:], uint32(len(block.Data)))

	payload := append(header, block.Data...)
	return (&Frame{Type: MsgDataBlock, Payload: payload}).WriteTo(w)
}

// ReadBlock decodes a DataBlock from the payload of a MsgDataBlock frame.
func ReadBlock(payload []byte) (DataBlock, error) {
	if len(payload) < 20 {
		return DataBlock{}, fmt.Errorf("block payload too short: %d bytes (need ≥20)", len(payload))
	}
	offset   := binary.BigEndian.Uint64(payload[0:])
	checksum := binary.BigEndian.Uint64(payload[8:])
	dataLen  := binary.BigEndian.Uint32(payload[16:])

	if int(dataLen) > len(payload)-20 {
		return DataBlock{}, fmt.Errorf("block truncated: dataLen=%d but only %d bytes remain", dataLen, len(payload)-20)
	}
	data := make([]byte, dataLen)
	copy(data, payload[20:20+dataLen])
	return DataBlock{Offset: offset, Checksum: checksum, Data: data}, nil
}

// ── Conn wrapper ──────────────────────────────────────────────────────────────

// Conn wraps net.Conn with per-operation deadline support and helper methods.
type Conn struct {
	net.Conn
	Timeout time.Duration
}

func (c *Conn) setWriteDeadline() {
	if c.Timeout > 0 {
		_ = c.Conn.SetWriteDeadline(time.Now().Add(c.Timeout))
	}
}

func (c *Conn) setReadDeadline() {
	if c.Timeout > 0 {
		_ = c.Conn.SetReadDeadline(time.Now().Add(c.Timeout))
	}
}

// WriteFrame sets a write deadline and sends a Frame.
func (c *Conn) WriteFrame(f *Frame) error {
	c.setWriteDeadline()
	return f.WriteTo(c.Conn)
}

// ReadFrame sets a read deadline and reads one Frame.
func (c *Conn) ReadFrame() (*Frame, error) {
	c.setReadDeadline()
	return ReadFrom(c.Conn)
}

// WriteJSON serialises v to JSON and sends it as msgType frame.
func (c *Conn) WriteJSON(msgType uint32, v any) error {
	payload, err := json.Marshal(v)
	if err != nil {
		return fmt.Errorf("marshal %T: %w", v, err)
	}
	c.setWriteDeadline()
	return (&Frame{Type: msgType, Payload: payload}).WriteTo(c.Conn)
}

// WriteBlock encodes and sends one DataBlock.
func (c *Conn) WriteBlock(block DataBlock) error {
	c.setWriteDeadline()
	return WriteBlock(c.Conn, block)
}

// decodeJSON is a package-level helper used by client and server.
func decodeJSON(data []byte, v any) error {
	return json.Unmarshal(data, v)
}
