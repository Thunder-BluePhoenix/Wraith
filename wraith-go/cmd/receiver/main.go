// wraith-receiver listens for an incoming snapshot from wraith-transmitter,
// writes the received bytes to disk, and exits.
//
// Usage (start before the transmitter):
//
//	wraith-receiver --listen :9999 --output /tmp/received.pb
//
// The Python orchestrator (Phase 5) starts the receiver via SSH before
// triggering the transmitter on the source machine.
package main

import (
	"flag"
	"fmt"
	"log"
	"os"
	"time"

	"github.com/wraith/transfer/pkg/transport"
)

func main() {
	listenAddr := flag.String("listen", ":9999", "Address:port to listen on (e.g. :9999 or 0.0.0.0:9999)")
	outputPath := flag.String("output", "received.pb", "Path to write the received snapshot")
	timeout    := flag.Duration("timeout", 10*time.Minute, "Per-operation I/O timeout")
	flag.Parse()

	rx, err := transport.NewReceiver(*listenAddr, *timeout)
	if err != nil {
		log.Fatalf("create receiver: %v", err)
	}
	defer rx.Close()

	log.Printf("ready — waiting for snapshot on %s", rx.Addr())

	data, hs, err := rx.AcceptSnapshot()
	if err != nil {
		log.Fatalf("receive snapshot: %v", err)
	}

	// Write with 0600: snapshot contains full process memory and must be treated
	// as sensitive data (see phase7.md security section).
	if err := os.WriteFile(*outputPath, data, 0600); err != nil {
		log.Fatalf("write snapshot to %s: %v", *outputPath, err)
	}

	fmt.Printf("\nSnapshot received:\n")
	fmt.Printf("  source     : %s (PID %d)\n", hs.SourceHostname, hs.SourcePid)
	fmt.Printf("  arch       : %s\n", hs.Arch)
	fmt.Printf("  size       : %d bytes (%d MB)\n", len(data), len(data)/1024/1024)
	fmt.Printf("  version    : %s\n", hs.SnapshotVersion)
	fmt.Printf("  saved to   : %s\n", *outputPath)
}
