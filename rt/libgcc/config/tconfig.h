/* Bennett's standalone x86-64 libgcc configuration, not an upstream file.
 * Values match gcc 13 -fbuilding-libgcc -dM on x86_64-linux-gnu.
 * Only SF/DF/XF complex helpers are built. */
#if !defined(__x86_64__) || __LDBL_MANT_DIG__ != 64
#error This configuration requires x86-64 with x87 long double
#endif
#define inhibit_libc 1
#define MIN_UNITS_PER_WORD 8
#define __LIBGCC_HAS_SF_MODE__ 1
#define __LIBGCC_HAS_DF_MODE__ 1
#define __LIBGCC_HAS_XF_MODE__ 1
#define __LIBGCC_SF_MANT_DIG__ __FLT_MANT_DIG__
#define __LIBGCC_DF_MANT_DIG__ __DBL_MANT_DIG__
#define __LIBGCC_XF_MANT_DIG__ __LDBL_MANT_DIG__
#define __LIBGCC_SF_FUNC_EXT__ f
#define __LIBGCC_DF_FUNC_EXT__
#define __LIBGCC_XF_FUNC_EXT__ l
#define __LIBGCC_SF_EXCESS_PRECISION__ 0
#define __LIBGCC_DF_EXCESS_PRECISION__ 0
#define __LIBGCC_XF_EXCESS_PRECISION__ 0
#define __LIBGCC_HAVE_HWDBL__ 1
#define __LIBGCC_SF_MAX__ __FLT_MAX__
#define __LIBGCC_SF_MIN__ __FLT_MIN__
#define __LIBGCC_SF_EPSILON__ __FLT_EPSILON__
#define __LIBGCC_DF_MAX__ __DBL_MAX__
#define __LIBGCC_DF_MIN__ __DBL_MIN__
#define __LIBGCC_DF_EPSILON__ __DBL_EPSILON__
#define __LIBGCC_XF_MAX__ __LDBL_MAX__
#define __LIBGCC_XF_MIN__ __LDBL_MIN__
#define __LIBGCC_XF_EPSILON__ __LDBL_EPSILON__
/* GCC-only mode aliases in unused integer-helper declarations. */
#define __libgcc_cmp_return__ DI
#define __libgcc_shift_count__ DI
