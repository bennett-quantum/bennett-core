# Vendored: musl 1.2.5

`musl-1.2.5/` is the complete, unmodified upstream release archive from
<https://musl.libc.org/releases/musl-1.2.5.tar.gz> (2024-03-01).
Archive SHA-256: `a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4`.
Licence: `musl-1.2.5/COPYRIGHT` (MIT, with the upstream notices for individual components).

Authenticated on 2026-11-17 against the release's detached OpenPGP signature from
<https://musl.libc.org/releases/musl-1.2.5.tar.gz.asc>, using the maintainer key published at
<https://musl.libc.org/musl.pub>. GnuPG reports a good signature and `VALIDSIG` with fingerprint
`836489290BB6B70F99FFDA0556BCDB593020450F` (identity `musl libc <musl@libc.org>`), dated
2024-03-01 02:08:50 UTC. Key identity is anchored in that upstream HTTPS download; no local
web-of-trust certification is claimed. The authenticated archive has the SHA-256 above.

The complete extracted archive matches the vendored tree: all **2,697 regular files** byte for byte,
their executable flags, entry types and all **235 directories**; no extra or missing entries.
Reproduce from the repository root (only plain upstream downloads):

```sh
auth=$(mktemp -d)
curl --fail --location https://musl.libc.org/releases/musl-1.2.5.tar.gz -o "$auth/musl-1.2.5.tar.gz"
curl --fail --location https://musl.libc.org/releases/musl-1.2.5.tar.gz.asc -o "$auth/musl-1.2.5.tar.gz.asc"
curl --fail --location https://musl.libc.org/musl.pub -o "$auth/musl.pub"
mkdir -m 700 "$auth/gnupg"
gpg --homedir "$auth/gnupg" --batch --import "$auth/musl.pub"
gpg --homedir "$auth/gnupg" --batch --status-fd 1 --verify \
  "$auth/musl-1.2.5.tar.gz.asc" "$auth/musl-1.2.5.tar.gz"
sha256sum "$auth/musl-1.2.5.tar.gz"
tar -xzf "$auth/musl-1.2.5.tar.gz" -C "$auth"
diff --recursive --no-dereference "$auth/musl-1.2.5" rt/musl/musl-1.2.5
python3 - "$auth/musl-1.2.5" rt/musl/musl-1.2.5 <<'PYTHON'
from pathlib import Path
import stat
import sys
up, vend = map(Path, sys.argv[1:])
a = {p.relative_to(up): p for p in up.rglob('*')}
b = {p.relative_to(vend): p for p in vend.rglob('*')}
assert a.keys() == b.keys()
for key in a:
    x, y = a[key], b[key]
    assert stat.S_IFMT(x.lstat().st_mode) == stat.S_IFMT(y.lstat().st_mode), key
    if x.is_file():
        assert x.read_bytes() == y.read_bytes(), key
        assert bool(x.stat().st_mode & 0o111) == bool(y.stat().st_mode & 0o111), key
print(sum(p.is_file() for p in a.values()), 'files;',
      sum(p.is_dir() for p in a.values()), 'directories; identical')
PYTHON
```

To update, replace the directory with an authenticated unmodified release and record the
signature, key fingerprint, archive identity and complete-tree comparison here.

Bennett's overlays and build are outside that directory. The upstream Makefile chooses the source list;
the build substitutes the generic C implementation for x86-64 assembly or inline assembly wherever musl
supplies one. Upstream sources and headers are never patched. Build products stay outside the vendor tree.
