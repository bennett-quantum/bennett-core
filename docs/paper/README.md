# Bennett model paper

[Read the paper (PDF)](bennett-model.pdf).

The paper develops the mathematics behind ordinary code driving quantum
allocation, gates, measurement and release by partial trace. Today's
`apply` uses the compute, copy and undo idea described in the paper.
Section notes mark the earlier `mark` and `rewind` forms, removed on
17 November 2026. The theorems for that design still hold, the forms can
be recovered from commit `f81a70a`, and the paper's open problems stand.

## Build

Use pdfLaTeX from `docs/paper`. Obtain `pf2.sty` from Leslie Lamport's
[LaTeX packages page](https://lamport.azurewebsites.net/latex/latex.html)
and save it in `../../.tmp/paper`. The style file carries no explicit
redistribution licence and is supplied separately by its author.

```sh
mkdir -p ../../.tmp/paper
# Save the author's pf2.sty in ../../.tmp/paper before running these passes.
(
  set -e
  export TEXINPUTS=".:../../.tmp/paper:"
  for pass in 1 2 3; do
    pdflatex -no-shell-escape -halt-on-error -interaction=nonstopmode \
      -output-directory=../../.tmp/paper ./bennett-model.tex
  done
)
```

The rebuilt PDF is `../../.tmp/paper/bennett-model.pdf`.
