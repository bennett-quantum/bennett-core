#!/usr/bin/env python3
"""Extract the global-write experiment's derived tables; logs stay outside the source tree.

Usage: python3 experiments/journal-content/global-tables.py .tmp/global-final.txt
Fits use Python's statistics.linear_regression over reps 1,2,4,8. NA means a constant x,
not a zero slope. All journal counts classify the apply callee; world costs are separate.
"""
import csv
from pathlib import Path
import re
import statistics
import sys


def slope(x, y):
    return f"{statistics.linear_regression(x, y).slope:.6f}" if len(set(x)) > 1 else "NA"


def main():
    text = Path(sys.argv[1]).read_text()
    assert "test result: ok. 3 passed; 0 failed" in text, "use the completed three-test run"
    lines = [m.group(0) for line in text.splitlines() if (m := re.search(r"(?:global|world|overlay)\t.*", line))]
    tables = {}
    for kind, count in [("global", 224), ("world", 28), ("overlay", 36)]:
        rows = list(csv.reader((line for line in lines if line.startswith(kind + "\t")), delimiter="\t"))
        assert rows[0][1] == "work"
        assert len(rows) == count + 1, (kind, len(rows))
        tables[kind] = [dict(zip(rows[0], row, strict=True)) for row in rows[1:]]
    fits = ["fit\twork\talign\tmode\tmax_words_per_step\tirreducible_per_step\tirreducible_per_global_byte\tmax_words_per_global_byte\tevaluations_per_branch_step"]
    grouped = {}
    for row in tables["global"]:
        grouped.setdefault((row["work"], row["align"], row["mode"]), []).append(row)
    for (work, align, mode), rows in grouped.items():
        rows.sort(key=lambda row: int(row["reps"]))
        # The means are integer sums over 256 branches; recover their exact binary fractions
        # from the six displayed decimals before fitting (roundoff is less than half of 1/256).
        steps = [round(float(r["mean_steps"]) * 256) / 256 for r in rows]
        byte_counts = [round(float(r["mean_global_bytes"]) * 256) / 256 for r in rows]
        words = [int(r["max_words"]) for r in rows]
        residual = [int(r["irreducible"]) for r in rows]
        evaluations = [int(r["evaluations"]) / 256 for r in rows]
        fits.append("\t".join(["fit", work, align, mode, slope(steps, words), slope(steps, residual), slope(byte_counts, residual), slope(byte_counts, words), slope(steps, evaluations)]))
    output = Path(__file__).with_name("global.tsv")
    output.write_text("# Global-write oracle journals and actual worlds/overlay costs; 256 branches in global/world rows.\n# Counts are positions unless named instances; evaluations exclude cleanup/scratch/lookup costs.\n# Overlay sweep has 8 basis configurations (two reads; peak worlds printed), with 0, 1 MiB, 4 MiB extra classical memory.\n# mean_*bytes count executed byte writes including repeated addresses; overlay counts differ from base.\n" + "\n".join(lines + fits) + "\n")
    print(f"{output.name}: 224 classifications, 28 worlds, 36 overlay rows, 56 fits")


if __name__ == "__main__":
    main()
