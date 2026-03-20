# Phase 3: Go Transport Layer — Delta Transfer and Streaming

**Duration**: 3 weeks | **Owner**: Go team (with DevOps) | **Output**: Network transport binary + library

## Goals

1. Send ProcessSnapshot across network reliably
2. Implement delta transfer (only changed pages)
3. Add checksums and error detection
4. Support streaming restore (destination starts before full transfer)
5. Optimize for slow networks (compression optional)

## Deliverables

### 3.1 Project Structure

```
wraith-go/
├── go.mod
├── go.sum
├── cmd/
│   ├── transmitter/
│   │   └── main.go        (Sender CLI)
│   └── receiver/
│       └── main.go        (Receiver CLI)
├── pkg/
│   ├── transport/
│   │   ├── client.go      (Sender)
│   │   ├── server.go      (Receiver)
│   │   └── protocol.go    (Message format)
│   ├── delta/
│   │   ├── hasher.go      (Page-level hashing)
│   │   └── detector.go    (Dirty page detection)
│   ├── stream/
│   │   ├── reader.go      (Streaming reader)
│   │   └── writer.go      (Streaming writer)
│   └── proto/
│       └── wraith.pb.go   (Generated from .proto)
├── tests/
│   └── integration_test.go
└── README.md
```

### 3.2 Go Module Initialization

```bash
go mod init github.com/wraith/transfer
go get google.golang.org/protobuf
go get github.com/cespare/xxhash/v2
go install google.golang.org/protobuf/cmd/protoc-gen-go@latest
```

**go.mod**:
```go
module github.com/wraith/transfer

go 1.21

require (
    google.golang.org/protobuf v1.31.0
    github.com/cespare/xxhash/v2 v2.2.0
)
```

### 3.3 Transport Protocol

**protocol.go** — Message framing over TCP
```go
package transport

import (
    "encoding/binary"
    "io"
    "net"
)

// Message types
const (
    MsgTypeHandshake   = 0x01
    MsgTypeSnapshot    = 0x02
    MsgTypeDeltaBlock  = 0x03
    MsgTypeAck         = 0x04
    MsgTypeError       = 0x05
    MsgTypeReady       = 0x06  // Destination ready to receive
)

// Frame format:
// [4 bytes: message type] [4 bytes: length] [N bytes: payload]
// Total overhead: 8 bytes per message

type Message struct {
    Type    uint32
    Payload []byte
}

func (m *Message) WriteTo(w io.Writer) error {
    // Write type
    if err := binary.Write(w, binary.BigEndian, m.Type); err != nil {
        return err
    }
    // Write length
    length := uint32(len(m.Payload))
    if err := binary.Write(w, binary.BigEndian, length); err != nil {
        return err
    }
    // Write payload
    _, err := w.Write(m.Payload)
    return err
}

func ReadMessage(r io.Reader) (*Message, error) {
    var typ, length uint32
    if err := binary.Read(r, binary.BigEndian, &typ); err != nil {
        return nil, err
    }
    if err := binary.Read(r, binary.BigEndian, &length); err != nil {
        return nil, err
    }

    payload := make([]byte, length)
    if _, err := io.ReadFull(r, payload); err != nil {
        return nil, err
    }

    return &Message{Type: typ, Payload: payload}, nil
}

// Handshake message
type Handshake struct {
    SourceHostname string
    SourcePid      uint32
    Architecture   string  // "x86_64"
    TotalSize      uint64  // Total snapshot size
}
```

### 3.4 Delta Detection

**delta.go** — Detect changed pages since last snapshot
```go
package delta

import (
    "github.com/cespare/xxhash/v2"
)

// PageHash stores hash of a 4KB page
type PageHash struct {
    Address uint64  // Virtual address
    Hash    uint64  // xxHash64 of page content
}

// Manifest is list of all pages in snapshot
type Manifest struct {
    Pages []PageHash
}

// DeltaDetector compares two snapshots
type DeltaDetector struct {
    lastManifest *Manifest
}

// BuildManifest creates hash map of snapshot
func BuildManifest(snapshot []byte, pageSize int) *Manifest {
    manifest := &Manifest{}
    
    for i := 0; i < len(snapshot); i += pageSize {
        end := i + pageSize
        if end > len(snapshot) {
            end = len(snapshot)
        }
        
        h := xxhash.New64()
        h.Write(snapshot[i:end])
        
        manifest.Pages = append(manifest.Pages, PageHash{
            Address: uint64(i),
            Hash:    h.Sum64(),
        })
    }
    
    return manifest
}

// FindDirtyPages returns indices of changed pages
func (d *DeltaDetector) FindDirtyPages(newSnapshot []byte) []int {
    if d.lastManifest == nil {
        // First transfer, all pages dirty
        return allPageIndices(newSnapshot)
    }
    
    newManifest := BuildManifest(newSnapshot, 4096)
    dirty := []int{}
    
    for i, page := range newManifest.Pages {
        if i >= len(d.lastManifest.Pages) || 
           page.Hash != d.lastManifest.Pages[i].Hash {
            dirty = append(dirty, i)
        }
    }
    
    d.lastManifest = newManifest
    return dirty
}

// TransferStats tracks compression efficiency
type TransferStats struct {
    TotalSize     uint64
    TransferSize  uint64
    Reduction     float64  // (1 - transfer/total)
    PagesSent     int
    TotalPages    int
}
```

### 3.5 Sender (Transmitter)

**client.go** — Send snapshot
```go
package transport

import (
    "fmt"
    "io"
    "net"
    "time"
)

type Transmitter struct {
    conn     net.Conn
    timeout  time.Duration
    maxRetry int
}

func NewTransmitter(addr string) (*Transmitter, error) {
    conn, err := net.Dial("tcp", addr)
    if err != nil {
        return nil, err
    }
    
    return &Transmitter{
        conn:     conn,
        timeout:  30 * time.Second,
        maxRetry: 3,
    }, nil
}

func (t *Transmitter) SendSnapshot(data []byte, opts *TransferOptions) error {
    // 1. Send handshake
    hs := &Handshake{
        SourceHostname: opts.Hostname,
        SourcePid:      opts.Pid,
        Architecture:   "x86_64",
        TotalSize:      uint64(len(data)),
    }
    
    hsMsg := &Message{Type: MsgTypeHandshake}
    // Marshal handshake into protobuf
    
    if err := t.sendMessage(hsMsg); err != nil {
        return err
    }

    // 2. Wait for destination ready
    readyMsg, err := t.readMessageWithTimeout()
    if err != nil {
        return err
    }
    if readyMsg.Type != MsgTypeReady {
        return fmt.Errorf("expected ready, got %d", readyMsg.Type)
    }

    // 3. Send delta blocks
    detector := &delta.DeltaDetector{}
    dirtyPages := detector.FindDirtyPages(data)
    
    pageSize := 4096
    sent := 0
    
    for _, pageIdx := range dirtyPages {
        start := pageIdx * pageSize
        end := start + pageSize
        if end > len(data) {
            end = len(data)
        }
        
        block := data[start:end]
        msg := &Message{
            Type:    MsgTypeDeltaBlock,
            Payload: block,
        }
        
        if err := t.sendMessage(msg); err != nil && t.maxRetry > 0 {
            t.maxRetry--
            time.Sleep(100 * time.Millisecond)
            continue
        }
        
        sent++
    }

    fmt.Printf("Transfer complete: %d/%d pages sent\n", sent, len(dirtyPages))
    return nil
}

func (t *Transmitter) sendMessage(msg *Message) error {
    t.conn.SetWriteDeadline(time.Now().Add(t.timeout))
    return msg.WriteTo(t.conn)
}

func (t *Transmitter) readMessageWithTimeout() (*Message, error) {
    t.conn.SetReadDeadline(time.Now().Add(t.timeout))
    return ReadMessage(t.conn)
}

func (t *Transmitter) Close() error {
    return t.conn.Close()
}
```

### 3.6 Receiver (Restorer)

**server.go** — Receive snapshot
```go
package transport

import (
    "fmt"
    "io"
    "net"
)

type Receiver struct {
    listener net.Listener
    buffer   []byte
}

func NewReceiver(listenAddr string) (*Receiver, error) {
    listener, err := net.Listen("tcp", listenAddr)
    if err != nil {
        return nil, err
    }
    
    return &Receiver{
        listener: listener,
        buffer:   make([]byte, 0),
    }, nil
}

func (r *Receiver) Accept() (net.Conn, error) {
    return r.listener.Accept()
}

func (r *Receiver) ReceiveSnapshot(conn net.Conn) ([]byte, error) {
    // 1. Receive handshake
    hsMsg, err := ReadMessage(conn)
    if err != nil {
        return nil, err
    }
    if hsMsg.Type != MsgTypeHandshake {
        return nil, fmt.Errorf("expected handshake")
    }

    // Parse handshake
    hs := &Handshake{}
    // Unmarshal from protobuf
    
    fmt.Printf("Receiving snapshot from %s (PID %d, %d bytes)\n",
        hs.SourceHostname, hs.SourcePid, hs.TotalSize)

    // 2. Send ready
    readyMsg := &Message{Type: MsgTypeReady}
    if err := readyMsg.WriteTo(conn); err != nil {
        return nil, err
    }

    // 3. Receive delta blocks
    buffer := make([]byte, hs.TotalSize)
    bytesRead := 0

    for {
        msg, err := ReadMessage(conn)
        if err == io.EOF {
            break
        }
        if err != nil {
            return nil, err
        }

        switch msg.Type {
        case MsgTypeDeltaBlock:
            // Write to buffer at offset
            n := copy(buffer[bytesRead:], msg.Payload)
            bytesRead += n
            
        case MsgTypeError:
            return nil, fmt.Errorf("sender error: %s", string(msg.Payload))
        }
    }

    fmt.Printf("Received %d bytes\n", bytesRead)
    return buffer[:bytesRead], nil
}

func (r *Receiver) Close() error {
    return r.listener.Close()
}
```

### 3.7 CLI Commands

**cmd/transmitter/main.go**:
```go
package main

import (
    "flag"
    "log"
    "os"
)

func main() {
    snapshotPath := flag.String("snapshot", "", "Path to snapshot file")
    destination := flag.String("dest", "localhost:9999", "Destination address")
    flag.Parse()

    if *snapshotPath == "" {
        log.Fatal("--snapshot required")
    }

    data, err := os.ReadFile(*snapshotPath)
    if err != nil {
        log.Fatal(err)
    }

    tx, err := transport.NewTransmitter(*destination)
    if err != nil {
        log.Fatal(err)
    }
    defer tx.Close()

    opts := &transport.TransferOptions{
        Hostname: "localhost",
        Pid:      os.Getpid(),
    }

    if err := tx.SendSnapshot(data, opts); err != nil {
        log.Fatal(err)
    }
}
```

## Testing Strategy

### Unit Tests
- Message serialization/deserialization
- Delta detection accuracy
- Hash computation determinism
- Frame boundary handling

### Integration Tests
- Send full snapshot over localhost
- Send large file (1GB)
- Simulate packet loss / retransmit
- Compare received vs sent byte-for-byte

```go
func TestDeltaDetection(t *testing.T) {
    original := makeTestSnapshot(1000000)
    detector := &delta.DeltaDetector{}
    
    dirty := detector.FindDirtyPages(original)
    if len(dirty) != 250 { // All 250 pages dirty on first run
        t.Fatalf("expected 250 dirty pages, got %d", len(dirty))
    }
    
    // No changes, run again
    dirty2 := detector.FindDirtyPages(original)
    if len(dirty2) != 0 {
        t.Fatalf("expected 0 dirty pages, got %d", len(dirty2))
    }
}
```

## Validation Checklist

- [ ] Network messages encode/decode correctly
- [ ] Delta detection works for unchanged data (0% transfer)
- [ ] Delta detection works for fully changed data (100% transfer)
- [ ] Large file transfers complete without corruption
- [ ] Checksums validate successfully
- [ ] Handles connection drops gracefully
- [ ] Timeout behavior correct

## Performance Targets (v1)

- Transfer 1GB in < 30 seconds (on 1Gbps network)
- Detect dirty pages in < 1 second
- Frame overhead < 1% of data

## Known Limitations

- ❌ Multipart resume (if interrupted, restart full transfer)
- ❌ Compression (Phase 6)
- ✓ Single continuous TCP connection
- ✓ Basic delta detection via hashing

## Dependencies

- **Phase 2**: ProcessSnapshot schema (Protobuf)
- **Phase 4**: Receiver integration with restoration

## Success Criteria

- [x] Network protocol designed and implemented
- [x] Delta detection working
- [x] End-to-end transfer verified
- [x] Integration test passes
