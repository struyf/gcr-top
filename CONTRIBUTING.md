# Contributing to gcr-top

Thank you for your interest in contributing to `gcr-top`! We welcome bug reports, feature suggestions, and pull requests.

## Prerequisites

Make sure you have the following installed:
* [Rust](https://www.rust-lang.org/tools/install) (stable toolchain, 1.74+)
* [Google Cloud SDK (`gcloud`)](https://cloud.google.com/sdk/docs/install) (configured and authenticated to test against Google Cloud Run APIs)

## Local Development Workflow

### 1. Clone the repository
```bash
git clone https://github.com/struyf/gcr-top.git
cd gcr-top
```

### 2. Build and run locally
Run the application directly in debug mode:
```bash
cargo run
```

To compile an optimized release binary:
```bash
cargo build --release
```

### 3. Code formatting and linting
Before submitting a pull request, ensure your code follows the Rust style guidelines:
```bash
# Check formatting
cargo fmt --all -- --check

# Run linter
cargo clippy --all-targets --all-features -- -D warnings
```

### 4. Running tests
Run the test suite locally:
```bash
cargo test
```

## Pull Request Guidelines

1. Create a feature branch from `main`: `git checkout -b feature/my-new-feature`.
2. Keep pull requests focused on a single change or fix.
3. Make sure `cargo test`, `cargo fmt`, and `cargo clippy` pass cleanly.
4. Open a pull request against `main` with a clear explanation of what changed and why.