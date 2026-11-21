#!/bin/sh
# Runs inside the checks container (see check.sh): fmt, clippy -D warnings and
# the workspace tests, ending with CHECKS_OK or CHECKS_FAILED. A file rather
# than an inline `sh -c '...'`, because Windows PowerShell mangles quotes in
# the arguments it hands to podman.exe.
cd /src || exit 1
echo "== fmt" &&
  cargo fmt --all -- --check &&
  echo "== clippy" &&
  cargo clippy --workspace --all-targets -- -D warnings &&
  echo "== tests" &&
  cargo test --workspace
if [ $? -eq 0 ]; then echo CHECKS_OK; else echo CHECKS_FAILED; fi
