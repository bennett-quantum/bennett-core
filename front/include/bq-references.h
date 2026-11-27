/* bq-references.h: included by force after bq.h in every unit bennett-cc compiles (docs/HANDLES.md, section 4;
 * docs/DESIGN.md, "The declaration check"). Not for programs to include.
 *
 * clang emits a declaration only where something uses it and fixes the symbol it stands for at that first use.
 * This array is that first use for each of the eleven primitives, ahead of the unit's own text, so that the unit's
 * LLVM IR shows the symbol each header declaration finally stands for. bennett-cc checks the entries, then deletes
 * the array before any optimiser pass. The order is that of docs/HANDLES.md, sections 5.3 to 5.6. */
#ifndef BENNETT_BQ_REFERENCES_H_INCLUDED
#define BENNETT_BQ_REFERENCES_H_INCLUDED
#pragma clang system_header
__attribute__((used)) static void (*const __bennett_references_[])(void) = {
    (void (*)(void))bq_alloc,         (void (*)(void))bq_free,
    (void (*)(void))bq_width,         (void (*)(void))bq_measure,
    (void (*)(void))bq_gate,          (void (*)(void))bq_rotate,
    (void (*)(void))bq_apply,         (void (*)(void))__bennett_effect,
    (void (*)(void))__bennett_exit,   (void (*)(void))__bennett_out_of_memory,
    (void (*)(void))__bennett_bad_argument,
};
#endif
