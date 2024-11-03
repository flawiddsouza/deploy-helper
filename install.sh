#!/bin/bash

set -e

REPO="flawiddsouza/deploy-helper"
LATEST_RELEASE=$(curl --silent "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')
ASSET_NAME="deploy-helper-$(uname | tr '[:upper:]' '[:lower:]')"

echo "Downloading $ASSET_NAME from release $LATEST_RELEASE..."

curl -L "https://github.com/$REPO/releases/download/$LATEST_RELEASE/$ASSET_NAME" -o /tmp/$ASSET_NAME
sudo mv /tmp/$ASSET_NAME /usr/local/bin/deploy-helper
sudo chmod +x /usr/local/bin/deploy-helper

echo "deploy-helper installed successfully!"
