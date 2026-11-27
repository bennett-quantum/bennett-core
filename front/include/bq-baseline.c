/* bq-baseline.c: the baseline of bennett-cc's declaration check (docs/DESIGN.md, "The declaration check").
 * bennett-cc compiles it with the same command and options as each unit, bq.h and bq-references.h included by
 * force, every LLVM pass disabled, and compares each primitive's declaration in the unit, and each direct call of
 * it, with the one here. Each primitive is called once, in a case of its own so that every call is reached. */
void __bennett_baseline_(int which, bq_reg r, uint64_t n, double angle, const bq_bit *controls, bq_fn *f,
                         const void *image, const bq_field *fields, char byte, int status);

void __bennett_baseline_(int which, bq_reg r, uint64_t n, double angle, const bq_bit *controls, bq_fn *f,
                         const void *image, const bq_field *fields, char byte, int status) {
    switch (which) {
    case 0:
        (void)bq_alloc(n);
        break;
    case 1:
        bq_free(r);
        break;
    case 2:
        (void)bq_width(r);
        break;
    case 3:
        (void)bq_measure(r, n, n);
        break;
    case 4:
        bq_gate(n, r, n, controls, n);
        break;
    case 5:
        bq_rotate(n, angle, r, n, controls, n);
        break;
    case 6:
        bq_apply(f, n, r, image, n, fields, n, controls, n);
        break;
    case 7:
        __bennett_effect(byte);
        break;
    case 8:
        __bennett_exit(status);
    case 9:
        __bennett_out_of_memory();
    default:
        __bennett_bad_argument();
    }
}
