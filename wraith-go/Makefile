.PHONY: build transmitter receiver test test-short clean tidy

BIN := ./bin

# ── Build ─────────────────────────────────────────────────────────────────────

transmitter:
	go build -o $(BIN)/wraith-transmitter ./cmd/transmitter

receiver:
	go build -o $(BIN)/wraith-receiver ./cmd/receiver

build: transmitter receiver

# ── Test ──────────────────────────────────────────────────────────────────────

test:
	go test ./... -v -count=1

# Skip the large-snapshot test (10 MB+) in fast mode.
test-short:
	go test ./... -short -count=1 -v

# ── Housekeeping ──────────────────────────────────────────────────────────────

tidy:
	go mod tidy

clean:
	rm -rf $(BIN)/

# ── Usage ─────────────────────────────────────────────────────────────────────
# After `make build`:
#
#   On destination machine (start first):
#     ./bin/wraith-receiver --listen :9999 --output /tmp/received.pb
#
#   On source machine:
#     ./bin/wraith-transmitter --snapshot /tmp/snapshot.pb --dest host:9999 --pid 12345
