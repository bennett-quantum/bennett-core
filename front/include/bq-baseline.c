/* bq-baseline.c: the baseline of bennett-cc's declaration check (docs/DESIGN.md, "The declaration check").
 * bennett-cc compiles it with the same command and options as each unit, bq.h and bq-references.h included by
 * force, every LLVM pass disabled, and compares each primitive's declaration in the unit, and each direct call of
 * it, with the one here. Each primitive is called once, in a case of its own so that every call is reached. Every
 * identifier here is reserved to the implementation, so that no macro of the user's options (-D, -include) can
 * change this text. */
void __bennett_baseline_(int __bennett_which_, bq_reg __bennett_r_, uint64_t __bennett_n_, double __bennett_angle_,
                         const bq_bit *__bennett_controls_, bq_fn *__bennett_f_, const void *__bennett_image_,
                         const bq_field *__bennett_fields_, char __bennett_byte_, int __bennett_status_);

void __bennett_baseline_(int __bennett_which_, bq_reg __bennett_r_, uint64_t __bennett_n_, double __bennett_angle_,
                         const bq_bit *__bennett_controls_, bq_fn *__bennett_f_, const void *__bennett_image_,
                         const bq_field *__bennett_fields_, char __bennett_byte_, int __bennett_status_) {
    switch (__bennett_which_) {
    case 0:
        (void)bq_alloc(__bennett_n_);
        break;
    case 1:
        bq_free(__bennett_r_);
        break;
    case 2:
        (void)bq_width(__bennett_r_);
        break;
    case 3:
        (void)bq_measure(__bennett_r_, __bennett_n_, __bennett_n_);
        break;
    case 4:
        bq_gate(__bennett_n_, __bennett_r_, __bennett_n_, __bennett_controls_, __bennett_n_);
        break;
    case 5:
        bq_rotate(__bennett_n_, __bennett_angle_, __bennett_r_, __bennett_n_, __bennett_controls_, __bennett_n_);
        break;
    case 6:
        bq_apply(__bennett_f_, __bennett_n_, __bennett_r_, __bennett_image_, __bennett_n_, __bennett_fields_,
                 __bennett_n_, __bennett_controls_, __bennett_n_);
        break;
    case 7:
        __bennett_effect(__bennett_byte_);
        break;
    case 8:
        __bennett_exit(__bennett_status_);
    case 9:
        __bennett_out_of_memory();
    default:
        __bennett_bad_argument();
    }
}
