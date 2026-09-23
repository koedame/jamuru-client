# Third-Party Licenses

This file lists the third-party components used in jamjam and their respective licenses.

## Rust Dependencies

### Audio

| Crate | License | Repository |
|-------|---------|------------|
| cpal | Apache-2.0 | https://github.com/RustAudio/cpal |
| rubato | MIT | https://github.com/HEnquist/rubato |
| hound | Apache-2.0 | https://github.com/ruuda/hound |

### Async Runtime

| Crate | License | Repository |
|-------|---------|------------|
| tokio | MIT | https://github.com/tokio-rs/tokio |
| tokio-test | MIT | https://github.com/tokio-rs/tokio |
| futures-util | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |

### Cryptography

| Crate | License | Repository |
|-------|---------|------------|
| aes-gcm | Apache-2.0 OR MIT | https://github.com/RustCrypto/AEADs |
| x25519-dalek | BSD-3-Clause | https://github.com/dalek-cryptography/x25519-dalek |
| ed25519-dalek | BSD-3-Clause | https://github.com/dalek-cryptography/curve25519-dalek |
| ed25519 | Apache-2.0 OR MIT | https://github.com/RustCrypto/signatures |
| signature | Apache-2.0 OR MIT | https://github.com/RustCrypto/traits |
| sha2 | MIT OR Apache-2.0 | https://github.com/RustCrypto/hashes |
| hkdf | MIT OR Apache-2.0 | https://github.com/RustCrypto/KDFs |

### TLS

| Crate | License | Repository |
|-------|---------|------------|
| rustls | Apache-2.0 OR MIT OR ISC | https://github.com/rustls/rustls |

### Networking

| Crate | License | Repository |
|-------|---------|------------|
| socket2 | MIT OR Apache-2.0 | https://github.com/rust-lang/socket2 |
| tokio-tungstenite | MIT | https://github.com/snapview/tokio-tungstenite |
| local-ip-address | MIT OR Apache-2.0 | https://github.com/EstebanBorai/local-ip-address |

### Serialization

| Crate | License | Repository |
|-------|---------|------------|
| serde | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| serde_json | MIT OR Apache-2.0 | https://github.com/serde-rs/json |
| bincode | MIT | https://github.com/bincode-org/bincode |
| data-encoding | MIT | https://github.com/ia0/data-encoding |

### Synchronization & Buffers

| Crate | License | Repository |
|-------|---------|------------|
| rtrb | MIT OR Apache-2.0 | https://github.com/mgeier/rtrb |
| parking_lot | MIT OR Apache-2.0 | https://github.com/Amanieu/parking_lot |

### Utilities

| Crate | License | Repository |
|-------|---------|------------|
| clap | MIT OR Apache-2.0 | https://github.com/clap-rs/clap |
| anyhow | MIT OR Apache-2.0 | https://github.com/dtolnay/anyhow |
| thiserror | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| uuid | MIT OR Apache-2.0 | https://github.com/uuid-rs/uuid |
| rand | MIT OR Apache-2.0 | https://github.com/rust-random/rand |
| libloading | ISC | https://github.com/nagisa/rust_libloading |
| sysinfo | MIT | https://github.com/GuillaumeGomez/sysinfo |
| tempfile (dev-dependency, CLI tests) | MIT OR Apache-2.0 | https://github.com/Stebalien/tempfile |

### Logging

| Crate | License | Repository |
|-------|---------|------------|
| tracing | MIT | https://github.com/tokio-rs/tracing |
| tracing-subscriber | MIT | https://github.com/tokio-rs/tracing |

## GUI Framework (Tauri)

| Crate | License | Repository |
|-------|---------|------------|
| tauri | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri |
| tauri-build | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri |
| tauri-plugin-shell | Apache-2.0 AND MIT | https://github.com/tauri-apps/plugins-workspace |
| tauri-plugin-deep-link | Apache-2.0 OR MIT | https://github.com/tauri-apps/plugins-workspace |
| axum (optional, `e2e-control` feature) | MIT | https://github.com/tokio-rs/axum |

## Frontend Dependencies (npm)

| Package | License | Repository |
|---------|---------|------------|
| react | MIT | https://github.com/facebook/react |
| react-dom | MIT | https://github.com/facebook/react |
| i18next | MIT | https://github.com/i18next/i18next |
| react-i18next | MIT | https://github.com/i18next/react-i18next |
| i18next-browser-languagedetector | MIT | https://github.com/i18next/i18next-browser-languageDetector |
| @tauri-apps/api | MIT OR Apache-2.0 | https://github.com/tauri-apps/tauri |
| @tauri-apps/plugin-deep-link | MIT OR Apache-2.0 | https://github.com/tauri-apps/plugins-workspace |
| vite | MIT | https://github.com/vitejs/vite |
| typescript | Apache-2.0 | https://github.com/microsoft/TypeScript |
| vitest | MIT | https://github.com/vitest-dev/vitest |
| storybook | MIT | https://github.com/storybookjs/storybook |
| @fontsource/inter | OFL-1.1 | https://github.com/fontsource/fontsource |
| @fontsource/roboto-mono | OFL-1.1 | https://github.com/fontsource/fontsource |

### Icons (copied into the source, not installed as a package)

| Source | License | Repository |
|--------|---------|------------|
| Lucide | ISC | https://github.com/lucide-icons/lucide |

The SVG path data of the icons in `ui/src/lib/icons.tsx` and in the component
files is copied from Lucide.

```
Copyright (c) 2026 Lucide Icons and Contributors
```

---

## License Texts

For the full text of each license, please refer to the respective repositories linked above.

### MIT License (Template)

```
MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### Apache License 2.0 (Summary)

Licensed under the Apache License, Version 2.0. You may obtain a copy at:
http://www.apache.org/licenses/LICENSE-2.0

### BSD-3-Clause License (Template)

```
BSD 3-Clause License

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice,
   this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its contributors
   may be used to endorse or promote products derived from this software
   without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### ISC License (Template)

```
ISC License

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

### SIL Open Font License 1.1 (Summary)

Applies to the Inter and Roboto Mono font files bundled via
`@fontsource/*` packages. Permits use, study, modification, and redistribution
(including bundling in this application), but the fonts may not be sold on
their own. Full text: https://openfontlicense.org/
