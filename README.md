# gcr-top
[![Crates.io](https://img.shields.io/crates/v/gcr-top.svg?style=flat-square)](https://crates.io/crates/gcr-top)
[![Release](https://img.shields.io/github/v/release/struyf/gcr-top?style=flat-square)](https://github.com/struyf/gcr-top/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square)](LICENSE)

A lightning-fast, terminal-based monitoring and operations dashboard (TUI) for Google Cloud Run, built in Rust with Ratatui.

gcr-top brings the ergonomics of tools like htop and k9s to serverless container fleets on Google Cloud Platform, supporting multi-region aggregated listing, real-time fuzzy filtering, and interactive canary traffic splitting.

## Features

* Multi-region aggregation (--region -): Query and monitor services across all Google Cloud locations in a single unified view.
* Dynamic regional routing: Automatically resolves specific regional API endpoints ({region}-run.googleapis.com) for low-latency operations and accurate mutation calls.
* Interactive traffic splitting (s): View all active and zero-traffic revisions, adjust percentage allocations in an interactive modal, and push live canary updates directly from the terminal.
* Instant search & filtering (/): Real-time, case-insensitive substring search across service names and deployment regions.
* Terminal safety & resilience: Custom panic hooks ensure your terminal raw mode and alternate screen are always cleanly restored, even during unexpected interruptions.
* Long-session auth handling: Seamlessly handles Google Cloud Application Default Credentials (ADC) token refreshing for long-running monitoring stations.

## Installation

### Pre-built binaries (Recommended)

Download the latest release binary for your platform from the GitHub Releases page (https://github.com/struyf/gcr-top/releases):

* Linux (x86_64 / ARM64)
* macOS (Apple Silicon / Intel)
* Windows (x86_64)

Extract the archive and move the binary to a directory in your $PATH:
```bash
tar -xzf gcr-top-v1.0.2-x86_64-unknown-linux-gnu.tar.gz
sudo mv gcr-top /usr/local/bin/
```

### Via Cargo (Crates.io)

```bash
cargo install gcr-top
```

### From source (via Cargo)

Ensure you have Rust and Cargo installed (>= 1.75 recommended):
```bash
git clone https://github.com/struyf/gcr-top.git
cd gcr-top
cargo build --release
sudo cp target/release/gcr-top /usr/local/bin/
```

## Authentication & Prerequisites

gcr-top uses standard Google Cloud Application Default Credentials (ADC). Ensure you are logged in with adequate permissions:
```bash
gcloud auth application-default login
```

## Usage

### Quick Start

Launch gcr-top by providing your Google Cloud Project ID:

```bash
gcr-top --project my-gcp-project-id
gcr-top --project $(gcloud config get-value project)
```

### Multi-Region Aggregation

To fetch services deployed across all regions simultaneously:
```bash
gcr-top --project $(gcloud config get-value project) --region -
```
## Keyboard Navigation

* Up / Down or k / j: Navigate through services list
* /: Open search / filter input bar
* Enter: Confirm search filter / Apply traffic split
* Esc: Clear search filter / Close modal window
* s: Open Traffic Split allocation modal for selected service
* Tab / Shift+Tab: Switch between revisions inside the Traffic Split modal
* + / -: Increase or decrease traffic percentage allocation
* q or Ctrl+C: Quit gcr-top

## Architecture & Design

* Engine: Built with ratatui and crossterm for low-latency terminal rendering.
* Client layer: Lightweight asynchronous HTTP client using reqwest (with rustls) querying the Cloud Run v2 REST APIs directly without heavyweight SDK overhead.
* Zero-allocation parsing: Efficient parsing of Cloud Run resource identifiers.

## Contributing

Contributions, bug reports, and suggestions are welcome!

1. Fork the repository
2. Create your feature branch (git checkout -b feature/my-feature)
3. Ensure formatting and clippy pass (cargo fmt --check && cargo clippy -- -D warnings)
4. Commit your changes (git commit -m 'feat: add my feature')
5. Push to the branch (git push origin feature/my-feature)
6. Open a Pull Request

## License

This project is licensed under the MIT License.
