
let components = (rustup component list --toolchain nightly)
if ($components | str contains "miri (installed)" or $components | str contains "miri-x86_64") {
    print "Miri is already installed and ready!"
} else {
    print "Miri component not found on nightly. Installing..."
    rustup component add miri --toolchain nightly
    cargo +nightly miri setup
}
