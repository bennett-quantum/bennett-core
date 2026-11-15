# Vendored: musl 1.2.5

`musl-1.2.5/` is the complete, unmodified upstream release archive from
<https://musl.libc.org/releases/musl-1.2.5.tar.gz> (2024-03-01).
Archive SHA-256: `a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4`.
Licence: `musl-1.2.5/COPYRIGHT` (MIT, with the upstream notices for individual components).

The supplied archive was hashed locally. Upstream's `.tar.gz.sha256` URL returned 404 on 2026-11-15;
this records the archive's identity, not independent authentication against a published upstream checksum.
To update, replace the directory with an unmodified release, check its identity, and record it here.

Bennett's overlays and build are outside that directory. The upstream Makefile chooses the source list;
the build substitutes the generic C implementation for x86-64 assembly or inline assembly wherever musl
supplies one. Upstream sources and headers are never patched. Build products stay outside the vendor tree.
