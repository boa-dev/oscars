# 1. Check if the nightly toolchain is installed
let toolchains = (rustup toolchain list)
if ($toolchains | str contains "nightly") {
    print "Nightly toolchain is already installed."
} else {
    print "Nightly toolchain not found. Installing..."
    rustup toolchain install nightly --profile minimal
}
