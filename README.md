# netperf (np)
A network performance measurement tool for TCP/UDP/QUIC protocols. Similar to iperf3 in usage.
QUIC testing uses the quinn QUIC implementation. Future support for Quiche is WIP.

## Differences to iperf3
- Some CLI options yet to be supported in np and some are WIP
- QUIC is supported in np
- SCTP is unsupported

## Usage
Server
------
'''
// binds to [::]:8080 by default
cargo run -- -s
'''
Client
'''
// connects to 127.0.0.1:8080 by default and test TCP streams
cargo run --
cargo run -- -c 127.0.0.1

// Test UDP performance
cargo run -- -u

// Test QUIC performance
cargo run -- -q

// Test with parallel streams using -P, period to test with -t
cargo run -- -u -P 2 -t 30
'''

More options available via help.

## Future
- Support for TCP congestion algorithm
- More performance metrics like rtt, retransmits, congestion window, etc
- More performance and configuration options for QUIC