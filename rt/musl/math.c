/* The C library's functions whose result IEEE 754 defines exactly, as the float runtime computes them. build.py
 * compiles this file once for each of them, with -DM_<name>, in place of musl's member of that name.
 *
 * The native program's glibc computes these as the IEEE operation (an x86 instruction, or software that is
 * exact), so their results, flags and signalling NaNs follow the hardware; musl's software gives the same
 * values but other flags (a spurious inexact from floor, lround, fmal, ...) and passes signalling NaNs through
 * unquieted. Each is the compiler's builtin of the same name under FENV_ACCESS, which bennett-front lowers to
 * the float runtime's routine (rt/softfloat/libgcc.c), the one a call of the builtin in the program itself
 * reaches. errno is not set, as by musl's. */
#pragma STDC FENV_ACCESS ON

#ifdef M_sqrt
double sqrt(double x) { return __builtin_sqrt(x); }
#endif

#ifdef M_sqrtf
float sqrtf(float x) { return __builtin_sqrtf(x); }
#endif

#ifdef M_sqrtl
long double sqrtl(long double x) { return __builtin_sqrtl(x); }
#endif

#ifdef M_fma
double fma(double x, double y, double z) { return __builtin_fma(x, y, z); }
#endif

#ifdef M_fmaf
float fmaf(float x, float y, float z) { return __builtin_fmaf(x, y, z); }
#endif

#ifdef M_fmal
long double fmal(long double x, long double y, long double z) { return __builtin_fmal(x, y, z); }
#endif

#ifdef M_rint
double rint(double x) { return __builtin_rint(x); }
#endif

#ifdef M_rintf
float rintf(float x) { return __builtin_rintf(x); }
#endif

#ifdef M_rintl
long double rintl(long double x) { return __builtin_rintl(x); }
#endif

#ifdef M_nearbyint
double nearbyint(double x) { return __builtin_nearbyint(x); }
#endif

#ifdef M_nearbyintf
float nearbyintf(float x) { return __builtin_nearbyintf(x); }
#endif

#ifdef M_nearbyintl
long double nearbyintl(long double x) { return __builtin_nearbyintl(x); }
#endif

#ifdef M_lrint
long lrint(double x) { return __builtin_lrint(x); }
#endif

#ifdef M_lrintf
long lrintf(float x) { return __builtin_lrintf(x); }
#endif

#ifdef M_lrintl
long lrintl(long double x) { return __builtin_lrintl(x); }
#endif

#ifdef M_llrint
long long llrint(double x) { return __builtin_llrint(x); }
#endif

#ifdef M_llrintf
long long llrintf(float x) { return __builtin_llrintf(x); }
#endif

#ifdef M_llrintl
long long llrintl(long double x) { return __builtin_llrintl(x); }
#endif

#ifdef M_round
double round(double x) { return __builtin_round(x); }
#endif

#ifdef M_roundf
float roundf(float x) { return __builtin_roundf(x); }
#endif

#ifdef M_roundl
long double roundl(long double x) { return __builtin_roundl(x); }
#endif

#ifdef M_lround
long lround(double x) { return __builtin_lround(x); }
#endif

#ifdef M_lroundf
long lroundf(float x) { return __builtin_lroundf(x); }
#endif

#ifdef M_lroundl
long lroundl(long double x) { return __builtin_lroundl(x); }
#endif

#ifdef M_llround
long long llround(double x) { return __builtin_llround(x); }
#endif

#ifdef M_llroundf
long long llroundf(float x) { return __builtin_llroundf(x); }
#endif

#ifdef M_llroundl
long long llroundl(long double x) { return __builtin_llroundl(x); }
#endif

#ifdef M_floor
double floor(double x) { return __builtin_floor(x); }
#endif

#ifdef M_floorf
float floorf(float x) { return __builtin_floorf(x); }
#endif

#ifdef M_floorl
long double floorl(long double x) { return __builtin_floorl(x); }
#endif

#ifdef M_ceil
double ceil(double x) { return __builtin_ceil(x); }
#endif

#ifdef M_ceilf
float ceilf(float x) { return __builtin_ceilf(x); }
#endif

#ifdef M_ceill
long double ceill(long double x) { return __builtin_ceill(x); }
#endif

#ifdef M_trunc
double trunc(double x) { return __builtin_trunc(x); }
#endif

#ifdef M_truncf
float truncf(float x) { return __builtin_truncf(x); }
#endif

#ifdef M_truncl
long double truncl(long double x) { return __builtin_truncl(x); }
#endif

#ifdef M_fmin
double fmin(double x, double y) { return __builtin_fmin(x, y); }
#endif

#ifdef M_fminf
float fminf(float x, float y) { return __builtin_fminf(x, y); }
#endif

#ifdef M_fminl
long double fminl(long double x, long double y) { return __builtin_fminl(x, y); }
#endif

#ifdef M_fmax
double fmax(double x, double y) { return __builtin_fmax(x, y); }
#endif

#ifdef M_fmaxf
float fmaxf(float x, float y) { return __builtin_fmaxf(x, y); }
#endif

#ifdef M_fmaxl
long double fmaxl(long double x, long double y) { return __builtin_fmaxl(x, y); }
#endif
