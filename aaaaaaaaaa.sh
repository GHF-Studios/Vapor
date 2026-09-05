# Clean the root-owned diagnostic leftovers.
sudo rm -rf .vapor

echo "rustup selected by your normal shell:"
command -v rustup
rustup --version

echo
echo "installing with EXACTLY the environment/arguments Vapor intends:"
RUSTUP_HOME="$PWD/.vapor/rustup-home" \
CARGO_HOME="$PWD/.vapor/cargo-home" \
"$(command -v rustup)" \
toolchain install 1.97.0 \
    --profile minimal \
    --no-self-update \
    --component rustfmt \
    --component clippy \
    --component rust-src \
    --component rust-analyzer

echo
echo "result:"
RUSTUP_HOME="$PWD/.vapor/rustup-home" \
CARGO_HOME="$PWD/.vapor/cargo-home" \
"$(command -v rustup)" toolchain list

ls -l \
  .vapor/rustup-home/toolchains/1.97.0-x86_64-unknown-linux-gnu/bin/cargo \
  .vapor/rustup-home/toolchains/1.97.0-x86_64-unknown-linux-gnu/bin/rustc \
  .vapor/rustup-home/toolchains/1.97.0-x86_64-unknown-linux-gnu/bin/rust-analyzer