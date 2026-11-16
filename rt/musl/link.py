#!/usr/bin/env python3
"""Compile one C program and resolve musl archive members, then emit its device program."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def run(args, **kwargs):
    return subprocess.run([str(a) for a in args], check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("-o", required=True, type=Path)
    parser.add_argument("--libdir", type=Path, default=ROOT / "target/musl")
    parser.add_argument("--front", type=Path, default=ROOT / "target/front/bennett-front")
    parser.add_argument("--arena-bytes", type=int, default=1 << 20)
    parser.add_argument("--stack-bytes", type=int, default=1 << 20)
    parser.add_argument("--region-budget", type=int, default=100000)
    parser.add_argument("--clang-flag", action="append", default=[])
    parser.add_argument("--keep", type=Path, help="retain intermediate files in this directory")
    parser.add_argument("--clock-start", type=int, default=0, help="run input: initial unsigned nanoseconds")
    parser.add_argument("--entropy-image", type=Path, help="run input: exact entropy bytes (default empty)")
    parser.add_argument("--file-image", type=Path, help="run input: JSON mapping absolute file paths to hex bytes")
    args = parser.parse_args()
    if not 0 <= args.clock_start < 1 << 64:
        parser.error("--clock-start must fit in an unsigned 64-bit integer")
    lib = args.libdir.resolve()
    args.o = args.o.resolve()
    args.o.parent.mkdir(parents=True, exist_ok=True)
    clang = os.environ.get("CLANG", "clang-18")
    flags = json.loads((lib / "cflags.json").read_text())
    if args.arena_bytes < 4096 or args.arena_bytes % 4096:
        parser.error("--arena-bytes must be a positive whole number of 4096-byte pages")
    if not (lib / "index.tsv").exists():
        run(["python3", HERE / "resolve.py", lib / "members.txt", lib / "index.tsv"])
    temporary = tempfile.TemporaryDirectory(dir=args.o.parent) if args.keep is None else None
    scratch = Path(temporary.name) if temporary else args.keep.resolve()
    scratch.mkdir(parents=True, exist_ok=True)
    common = ["-O0", "-Xclang", "-disable-O0-optnone", "-gline-tables-only", "-emit-llvm", "-c"]
    run([clang, *common, "-nostdinc", "-isystem", lib / "sysroot/include", "-I", ROOT / "front/include",
         *args.clang_flag, args.source.resolve(), "-o", scratch / "program.bc"])
    ir = run(["llvm-dis-18", scratch / "program.bc", "-o", "-"], capture_output=True, text=True).stdout
    match = re.search(r"^define\b[^\n]* @main\(([^\n]*)\)", ir, re.M)
    if not match or "..." in match[1]:
        parser.error("the program needs a defined nonvariadic int main")
    count = 0 if not match[1].strip() else len(match[1].split(","))
    if count > 3:
        parser.error("main has more than three parameters")
    # Inputs are data in the run image, never host calls made by the device. Keep them separately
    # compilable: the linked C functions consume these bytes with ordinary journalled loads/stores.
    entropy = args.entropy_image.read_bytes() if args.entropy_image else b""
    files = json.loads(args.file_image.read_text()) if args.file_image else {}
    if not isinstance(files, dict):
        parser.error("--file-image must be an object mapping absolute paths to hex strings")
    contents = []
    for path, data in files.items():
        if not isinstance(path, str) or not path.startswith("/") or "\0" in path or path.endswith("/") or any(x in ("", ".", "..") for x in path[1:].split("/")):
            parser.error("file-image paths must be canonical absolute regular-file paths")
        try:
            contents.append((path, bytes.fromhex(data)))
        except (ValueError, TypeError):
            parser.error("file-image values must be hex strings")
    file_paths = {path for path, _ in contents}
    if file_paths & {"/dev", "/tmp", "/dev/null", "/dev/zero", "/dev/random", "/dev/urandom", "/dev/console"}:
        parser.error("file-image paths must not replace built-in namespace entries")
    if any(str(parent) in file_paths for path in file_paths for parent in Path(path).parents):
        parser.error("a file-image path cannot be another file's parent directory")
    def cbytes(data):
        return '"' + ''.join(f"\\x{x:02x}" for x in data) + '"'
    inputs_c = ['#include <stdint.h>', '#include <fcntl.h>', '#include <unistd.h>', '#include <sys/stat.h>',
                f'const uint64_t __bennett_clock_start = {args.clock_start}ULL;',
                f'const uint64_t __bennett_entropy_size = {len(entropy)}ULL;',
                f'const unsigned char __bennett_entropy_image[] = {cbytes(entropy)};',
                'void __bennett_input_files(void) {']
    directories = {"/dev", "/tmp"}
    for path, data in contents:
        for directory in reversed(Path(path).parents):
            if str(directory) not in ("/", ".", "/dev") and str(directory) not in directories:
                inputs_c.append(f'if (mkdir({cbytes(str(directory).encode())}, 0755)) __builtin_trap();')
                directories.add(str(directory))
        inputs_c += ['{', f'int fd = open({cbytes(path.encode())}, O_CREAT|O_EXCL|O_WRONLY, 0644);',
                     f'if (fd < 0 || write(fd, {cbytes(data)}, {len(data)}) != {len(data)} || close(fd)) __builtin_trap();', '}']
    inputs_c.append('}')
    (scratch / "inputs.c").write_text('\n'.join(inputs_c))
    sources = [("runtime.c", "runtime.bc", [f"-DBENNETT_ARENA_BYTES={args.arena_bytes}"]),
               ("start.c", "start.bc", [f"-DMAIN_ARGS={count}"]),
               ("vfs.c", "vfs.bc", []), ("devices.c", "devices.bc", []), ("models.c", "models.bc", [])]
    for source, output, extra in sources:
        run([clang, *flags, *common, *extra, HERE / source, "-o", scratch / output])
    run([clang, *flags, *common, scratch / "inputs.c", "-o", scratch / "inputs.bc"])
    inputs = [scratch / output for _, output, _ in sources] + [scratch / "program.bc", scratch / "inputs.bc"]
    roots = []
    for _ in range(16):
        selected = run(["python3", HERE / "resolve.py", lib / "members.txt", lib / "index.tsv", *inputs, *roots],
                       capture_output=True, text=True).stdout.splitlines()
        run(["llvm-link-18", *inputs, *selected, "-o", scratch / "linked.bc"])
        run(["llvm-dis-18", scratch / "linked.bc", "-o", scratch / "linked.ll"])
        run(["python3", HERE / "weak.py", scratch / "linked.ll", scratch / "linked.ll"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        run(["llvm-as-18", scratch / "linked.ll", "-o", scratch / "linked.bc"])
        result = subprocess.run([str(args.front.resolve()), str(scratch / "linked.bc"), "-o", str(args.o),
                                 "--stack-bytes", str(args.stack_bytes), "--region-budget", str(args.region_budget)],
                                capture_output=True, text=True)
        if result.returncode == 0:
            return
        missing = re.search(r"^not handled yet: a call to @([^,]+), which has no definition", result.stderr, re.M)
        index = (lib / "index.tsv").read_text()
        if missing and missing[1] not in roots and re.search(r"\t(?:strong|weak)\t" + re.escape(missing[1]) + r"$", index, re.M):
            roots += ["-u", missing[1]]
            continue
        raise SystemExit(result.stderr)
    raise SystemExit("library resolution did not settle after 16 rounds")


if __name__ == "__main__":
    main()
