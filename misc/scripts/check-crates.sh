#!/usr/bin/env bash

set -euo pipefail

# MSRV
msrv="1.70.0"

is_msrv=false
version=""

# Check if "msrv" is passed as an argument
if [[ "$#" -gt 0 && "$1" == "msrv" ]]; then
    is_msrv=true
    version="+$msrv"
fi

# Check if MSRV
if [ "$is_msrv" == true ]; then
    # Install MSRV
    rustup install $msrv
    rustup component add clippy --toolchain $msrv
    rustup target add wasm32-unknown-unknown --toolchain $msrv
fi

buildargs=(
    "-p cdk"
    "-p cdk --no-default-features"
    "-p cdk --no-default-features --features wallet"
    "-p cdk --no-default-features --features mint"
    "-p cdk-redb"
    "-p cdk-redb --no-default-features --features wallet"
    "-p cdk-redb --no-default-features --features mint"
    "-p cdk-sqlite --no-default-features --features mint"
    "-p cdk-sqlite --no-default-features --features wallet"
    "-p cdk-cln"
    "-p cdk-axum"
    "--bin cdk-cli"
    "--bin cdk-mintd"
    "--examples"
)

for arg in "${buildargs[@]}"; do
    if [[ $version == "" ]]; then
        echo  "Checking '$arg' [default]"
    else
        echo  "Checking '$arg' [$version]"
    fi
    cargo $version check $arg
    if [[ $arg != *"--target wasm32-unknown-unknown"* ]]; then
        cargo $version test $arg
    fi
    cargo $version clippy $arg -- -D warnings
    echo
done
