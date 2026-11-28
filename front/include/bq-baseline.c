/* bq-baseline.c: the baseline of bennett-cc's declaration check (docs/DESIGN.md, "The declaration check").
 *
 * bennett-cc compiles this file with bq-prefix.h included by force, as in every unit, every LLVM pass disabled,
 * and of the user's command only the code-generation options its table names (front/src/BaselineOptions.h): no
 * environment of the user's or the host's, no configuration file, no include path, no macro, no other file. It
 * compares each primitive's declaration in the unit, and each direct call of it, with the one here. Each primitive
 * is called once, in a case of its own so that every call is reached. Nothing the user or the host controls
 * reaches this text, so the comparison is always with the declarations file's own declarations and calls. */
void __bennett_baseline_(int, struct __bennett_reg, __UINT64_TYPE__, double, const struct __bennett_bit *,
                         void (*)(void *), const void *, const struct __bennett_field *, char, int);

void __bennett_baseline_(int which, struct __bennett_reg r, __UINT64_TYPE__ n, double angle,
                         const struct __bennett_bit *controls, void (*f)(void *), const void *image,
                         const struct __bennett_field *fields, char byte, int status) {
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
