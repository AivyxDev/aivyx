# Building Aivyx Engine

## Prerequisites

- Rust stable toolchain (1.85+)
- Git (for fetching aivyx-core dependency)
- pkg-config, libssl-dev (Linux)

## Quick Build (Development)

```bash
cargo build --workspace
cargo test --workspace
```

## Release Build

```bash
cargo build --release -p aivyx-engine-cli
```

The binary is at `target/release/aivyx`.

## Cross-Compilation

### Using the build script

```bash
# x86_64 Linux (default)
./deploy/scripts/cross-build.sh

# ARM64 Linux
./deploy/scripts/cross-build.sh aarch64-linux

# Multiple targets
./deploy/scripts/cross-build.sh x86_64-linux aarch64-linux
```

Artifacts are placed in `./dist/`.

### Manual cross-compilation

#### ARM64 Linux (from x86_64)

```bash
# Install cross-compilation toolchain
sudo apt install gcc-aarch64-linux-gnu libc6-dev-arm64-cross

# Add Rust target
rustup target add aarch64-unknown-linux-gnu

# Build
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  cargo build --release --target aarch64-unknown-linux-gnu -p aivyx-engine-cli
```

#### macOS (native)

```bash
# On Intel Mac
cargo build --release -p aivyx-engine-cli

# On Apple Silicon (universal binary)
rustup target add x86_64-apple-darwin
cargo build --release --target x86_64-apple-darwin -p aivyx-engine-cli
cargo build --release --target aarch64-apple-darwin -p aivyx-engine-cli

# Create universal binary
lipo -create \
  target/x86_64-apple-darwin/release/aivyx \
  target/aarch64-apple-darwin/release/aivyx \
  -output aivyx-universal
```

## Tauri Desktop App

The desktop app lives in the `aivyx` repository at `apps/desktop/`.

### Prerequisites

- Node.js 22+
- System dependencies:

```bash
# Ubuntu/Debian
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev \
  libjavascriptcoregtk-4.1-dev

# macOS
xcode-select --install

# Windows
# Visual Studio Build Tools with C++ workload
```

### Build

```bash
# Build engine binary first (sidecar)
cargo build --release -p aivyx-engine-cli

# Copy sidecar to Tauri binaries directory
TARGET=$(rustc -vV | grep host | cut -d' ' -f2)
cp target/release/aivyx apps/desktop/src-tauri/binaries/aivyx-$TARGET

# Build desktop app
cd apps/desktop
npm ci
npx tauri build
```

### Output

| Platform | Format | Location |
|----------|--------|----------|
| Linux | `.deb` | `src-tauri/target/release/bundle/deb/` |
| Linux | `.AppImage` | `src-tauri/target/release/bundle/appimage/` |
| macOS | `.dmg` | `src-tauri/target/release/bundle/dmg/` |
| Windows | `.msi` | `src-tauri/target/release/bundle/msi/` |
| Windows | `.exe` (NSIS) | `src-tauri/target/release/bundle/nsis/` |

## Docker

```bash
# Engine image
docker build -f deploy/engine/Dockerfile -t aivyx-engine .

# Run
docker run -p 3000:8080 \
  -v ~/.aivyx:/home/aivyx/.aivyx \
  -e AIVYX_BEARER_TOKEN=your-token \
  aivyx-engine
```

## CI/CD

Automated builds run on Gitea Actions (self-hosted):

- **CI** (`ci.yml`): Every push/PR -- build, test, clippy, fmt, audit
- **Release** (`release.yml`): On `v*` tags -- multi-arch binaries, .deb, .AppImage, deploy

### Creating a release

```bash
git tag v1.1.0
git push origin v1.1.0
```

The release pipeline will:
1. Build x86_64 and aarch64 Linux binaries
2. Build Tauri desktop packages (.deb, .AppImage)
3. Generate SHA-256 checksums
4. Upload artifacts to the releases server
5. Deploy engine binary to production
