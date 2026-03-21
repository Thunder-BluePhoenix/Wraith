// wraith-transmitter sends a Protobuf snapshot file to a waiting wraith-receiver.
//
// Usage:
//
//	wraith-transmitter --snapshot /tmp/snapshot.pb --dest worker:9999 --pid 12345
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
	snapshot := flag.String("snapshot", "", "Path to snapshot file (required)")
	dest     := flag.String("dest", "localhost:9999", "Destination host:port")
	timeout  := flag.Duration("timeout", 5*time.Minute, "Per-operation I/O timeout")
	retries  := flag.Int("retries", 3, "Max per-block retries on transient errors")
	pid      := flag.Uint("pid", 0, "Source PID (included in handshake metadata)")
	version  := flag.String("version", "1.0", "Snapshot version string")
	flag.Parse()

	if *snapshot == "" {
		fmt.Fprintln(os.Stderr, "error: --snapshot is required")
		flag.Usage()
		os.Exit(1)
	}

	data, err := os.ReadFile(*snapshot)
	if err != nil {
		log.Fatalf("read snapshot %s: %v", *snapshot, err)
	}
	log.Printf("snapshot loaded: %s (%d bytes, %d MB)", *snapshot, len(data), len(data)/1024/1024)

	hostname, _ := os.Hostname()

	tx, err := transport.NewTransmitter(*dest, *timeout)
	if err != nil {
		log.Fatalf("connect to %s: %v", *dest, err)
	}
	defer tx.Close()

	opts := transport.TransferOptions{
		Hostname:        hostname,
		Pid:             uint32(*pid),
		SnapshotVersion: *version,
		Timeout:         *timeout,
		MaxRetries:      *retries,
	}

	stats, err := tx.SendSnapshot(data, opts)
	if err != nil {
		log.Fatalf("transfer failed: %v", err)
	}

	fmt.Printf("\nTransfer complete:\n")
	fmt.Printf("  pages sent  : %d / %d\n", stats.PagesSent, stats.TotalPages)
	fmt.Printf("  bytes sent  : %d MB\n", stats.BytesSent/1024/1024)
	fmt.Printf("  reduction   : %.1f%%\n", stats.Reduction*100)
}
