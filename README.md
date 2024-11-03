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
