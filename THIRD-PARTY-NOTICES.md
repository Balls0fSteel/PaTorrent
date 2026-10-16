# Third-party notices

PaTorrent's engine is a Rust port of libtorrent-rasterbar 2.0.11. Ported code is a derivative work of the projects below and remains under their terms. This file ships with every PaTorrent binary distribution.

Ported so far:
- `crates/core/src/bencode`, `crypto` (SHA-1, SHA-256, CRC32C), `util`, `metainfo`, and the digest operations in `types.rs`
- `crates/core/src/disk`, `engine` (session, torrent, peer connection, piece picker, peer list, choker, trackers, DHT and LSD glue, the UPnP and NAT-PMP / PCP port mappers), `net` (incl. the external-IP voter and the `enum_net` interface, route and gateway helpers), `proto` (peer wire, HTTP, tracker protocols, ut_metadata, ut_pex, LSD, UPnP, NAT-PMP / PCP, the XML scanner) and `resume`
- `crates/core/src/dht` (libtorrent's `kademlia/*`: node, routing table, rpc manager, lookups, storage, DoS blocker, node ids, DHT state) and `util/bloom_filter.rs`
- libtorrent's test torrents in `crates/core/tests/fixtures/torrents`

Each source file names its origin in a header comment.

PaTorrent is not affiliated with or endorsed by libtorrent, its authors, or any other project named here. `patorrent-nox --license` prints this file after PaTorrent's own LICENSE.md.

---

## libtorrent-rasterbar

https://github.com/arvidn/libtorrent, tag `v2.0.11`. Reference copy: `third_party/libtorrent`, which is not compiled.

```
Copyright (c) 2003-2020, Arvid Norberg
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:

    * Redistributions of source code must retain the above copyright
      notice, this list of conditions and the following disclaimer.
    * Redistributions in binary form must reproduce the above copyright
      notice, this list of conditions and the following disclaimer in
      the documentation and/or other materials provided with the distribution.
    * Neither the name of the author nor the names of its
      contributors may be used to endorse or promote products derived
      from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE
LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.
```

---

## puff (inflate)

Ported via libtorrent `src/puff.cpp` / `include/libtorrent/puff.hpp`. PaTorrent's port is an altered version.

```
Copyright (C) 2002, 2003 Mark Adler, all rights reserved

This software is provided 'as-is', without any express or implied
warranty.  In no event will the author be held liable for any damages
arising from the use of this software.

Permission is granted to anyone to use this software for any purpose,
including commercial applications, and to alter it and redistribute it
freely, subject to the following restrictions:

1. The origin of this software must not be misrepresented; you must not
   claim that you wrote the original software. If you use this software
   in a product, an acknowledgment in the product documentation would be
   appreciated but is not required.
2. Altered source versions must be plainly marked as such, and must not be
   misrepresented as being the original software.
3. This notice may not be removed or altered from any source distribution.

Mark Adler    madler@alumni.caltech.edu
```

---

## ed25519

Ported via libtorrent `src/ed25519/`. PaTorrent's port is an altered version.

```
Copyright (c) 2015 Orson Peters <orsonpeters@gmail.com>

This software is provided 'as-is', without any express or implied warranty. In no event will the
authors be held liable for any damages arising from the use of this software.

Permission is granted to anyone to use this software for any purpose, including commercial
applications, and to alter it and redistribute it freely, subject to the following restrictions:

1. The origin of this software must not be misrepresented; you must not claim that you wrote the
   original software. If you use this software in a product, an acknowledgment in the product
   documentation would be appreciated but is not required.

2. Altered source versions must be plainly marked as such, and must not be
   misrepresented as being the original software.

3. This notice may not be removed or altered from any source distribution.
```

---

## SHA-1

Ported via libtorrent `src/sha1.cpp`.

```
SHA-1 in C
By Steve Reid <sreid@sea-to-sky.net>
100% Public Domain
```

## SHA-256

Ported via libtorrent `src/sha256.cpp`.

```
SHA-256. Adapted from LibTomCrypt. This code is Public Domain
```

---

## Rust crates linked into the binaries

| Crate | Used by | License |
|---|---|---|
| `mio` | engine (all binaries): the network event loop (PLAN.md D2) | MIT |
| `log` (pulled in by `mio`) | same | MIT OR Apache-2.0 |
| `libc` (pulled in by `mio`) | same (Unix only) | MIT OR Apache-2.0 |
| `windows-sys` | engine (sparse files, and mio on Windows), Win32 GUI, `lt-swarm` (Windows only) | MIT OR Apache-2.0 |
| `windows-link` (pulled in by `windows-sys`) | same | MIT OR Apache-2.0 |

This table is kept current with every dependency change (dependency policy: docs/ARCHITECTURE.md).

## System libraries used at run time

HTTPS trackers use the operating system's TLS library, which is linked dynamically and not shipped with PaTorrent: SChannel (`secur32.dll`) on Windows, and the system OpenSSL (`libssl`, `libcrypto`; Apache-2.0 from 3.0, the OpenSSL/SSLeay license before) or LibreSSL on Linux and BSD.
