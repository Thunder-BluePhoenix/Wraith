// Package delta implements page-level dirty detection for snapshot delta transfer.
//
// The algorithm:
//  1. Split the snapshot into 4 KB pages.
//  2. Hash each page with xxHash-64 (fast, non-cryptographic).
//  3. Compare against the previous manifest.
//  4. Return only the offsets of pages whose hash changed.
//
// For a first-time transfer (no prior manifest) all pages are "dirty".
// For Phase 8.5 (live migration), the transmitter calls DirtyOffsets multiple
// times across pre-copy rounds — only changed pages are re-sent each round,
// reducing downtime to tens of milliseconds.
package delta

import (
	"github.com/cespare/xxhash/v2"
)

// pageHash stores the hash of one 4 KB page at a given byte offset.
type pageHash struct {
	offset uint64
	hash   uint64
}

// manifest is the ordered set of page hashes for an entire snapshot.
type manifest []pageHash

// Detector tracks page manifests across successive calls.
// It is not safe for concurrent use.
type Detector struct {
	prev manifest
}

// NewDetector returns a fresh Detector with no prior state.
func NewDetector() *Detector {
	return &Detector{}
}

// DirtyOffsets returns the byte offsets (within data) of all pages that differ
// from the previous call. On first call, every page is returned.
//
// The caller must not modify the returned slice.
func (d *Detector) DirtyOffsets(data []byte, pageSize int) []int {
	curr := buildManifest(data, pageSize)

	var dirty []int
	for i, p := range curr {
		if d.prev == nil || i >= len(d.prev) || p.hash != d.prev[i].hash {
			dirty = append(dirty, int(p.offset))
		}
	}

	d.prev = curr
	return dirty
}

// HashPage computes the xxHash-64 of a page.
// This is the same hash the Receiver uses to verify each DataBlock in transit.
func HashPage(data []byte) uint64 {
	return xxhash.Sum64(data)
}

// buildManifest produces an ordered page manifest for data.
func buildManifest(data []byte, pageSize int) manifest {
	numPages := (len(data) + pageSize - 1) / pageSize
	m := make(manifest, 0, numPages)
	for offset := 0; offset < len(data); offset += pageSize {
		end := offset + pageSize
		if end > len(data) {
			end = len(data)
		}
		m = append(m, pageHash{
			offset: uint64(offset),
			hash:   xxhash.Sum64(data[offset:end]),
		})
	}
	return m
}

// ChangeStats describes how many pages changed between two snapshots.
type ChangeStats struct {
	TotalPages int
	DirtyPages int
	CleanPages int
	// Reduction is the fraction of pages that did NOT change.
	Reduction float64
}

// Analyze compares prev and curr and returns change statistics.
// Useful for logging and observability.
func Analyze(prev, curr []byte, pageSize int) ChangeStats {
	pm := buildManifest(prev, pageSize)
	cm := buildManifest(curr, pageSize)

	dirty := 0
	for i, p := range cm {
		if i >= len(pm) || p.hash != pm[i].hash {
			dirty++
		}
	}

	total := len(cm)
	reduction := 0.0
	if total > 0 {
		reduction = 1.0 - float64(dirty)/float64(total)
	}
	return ChangeStats{
		TotalPages: total,
		DirtyPages: dirty,
		CleanPages: total - dirty,
		Reduction:  reduction,
	}
}
