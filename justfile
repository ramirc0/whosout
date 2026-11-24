set dotenv-path := "../.env"

units := env("XDG_CONFIG_HOME", home_directory() / ".config") / "systemd/user"

# List the recipes.
default:
    @just --list

# Check formatting, lint with clippy, build the docs and run the tests.
check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
    cargo test

# Run whosout with arguments. The Slack token comes from ../.env.
run *args:
    cargo run --release -- {{args}}

# Install the binary to ~/.local/bin and the systemd user units, then start the server and the daily timer.
install:
    cargo build --release
    mkdir -p ~/.local/bin "{{units}}" ~/.local/state/whosout
    install -m 755 target/release/whosout ~/.local/bin/
    install -m 644 systemd/* "{{units}}"
    systemctl --user daemon-reload
    systemctl --user enable --now whosout.service whosout-sync.timer
    systemctl --user restart whosout.service
