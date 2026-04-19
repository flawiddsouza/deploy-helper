# Deploy Helper

## Installation

To install the latest version of `deploy-helper` on macOS or Linux, run the following command:

```sh
curl -sSL https://raw.githubusercontent.com/flawiddsouza/deploy-helper/main/install.sh | bash
```

## Development

To set up a development environment for `deploy-helper`, follow these steps:

### Prerequisites

- Ensure you have [Rust](https://www.rust-lang.org/tools/install) installed on your system.

### Setup

1. Clone the repository:

   ```sh
   git clone https://github.com/flawiddsouza/deploy-helper.git
   cd deploy-helper
   ```

2. To build & run the application, use the following command::

    ```sh
    cargo run <deploy.yml>
    ```

    Replace `<deploy.yml>` with the path to your deployment configuration file.

## Install from Source

```sh
cargo install --path .
```

Builds and installs the `deploy-helper` binary to your Cargo bin directory, making it available globally.

## Deployment YAML

See [docs/deployment-yaml.md](docs/deployment-yaml.md).

## CLI Reference

See [docs/cli.md](docs/cli.md).

## Testing

Tests are integration tests located in `tests/integration_test.rs`. Each test runs the binary against a YAML deployment file in `test-ymls/` and compares the output to a corresponding `.out` file.

Tests run against two inventory targets in parallel: a local connection and a remote SSH connection. The SSH target requires Docker. A container is started automatically before the tests run and stopped after.

### Prerequisites

- [Docker](https://www.docker.com/) must be installed and running.

> **Note:** Tests are written for Linux. On Windows, run them via WSL:
> ```sh
> wsl -- bash -c "cargo test"
> ```

### Run all tests

```sh
cargo test
```

### Run a specific test

```sh
cargo test <test_name>
```

For example:

```sh
cargo test use_vars_in_task_name
```
