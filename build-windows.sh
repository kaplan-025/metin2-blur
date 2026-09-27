#!/bin/sh
# Tek dosya Windows exe uretir (ffmpeg gomulu, kurulum gerekmez).
# Gereken: rustup + x86_64-w64-mingw32-gcc (mingw-w64) + proje kokunde ffmpeg-win.exe
set -e
cd "$(dirname "$0")"

rustup target add x86_64-pc-windows-gnu
test -f ffmpeg-win.exe || { echo "ffmpeg-win.exe yok (bakiyesiz surum: exe'nin yanina ffmpeg.exe koy)"; exit 1; }

cargo test --release
cargo build --release --target x86_64-pc-windows-gnu
cp target/x86_64-pc-windows-gnu/release/metin2-blur.exe Metin2Blur.exe
ls -lh Metin2Blur.exe
echo "tamam: $(pwd)/Metin2Blur.exe"
