/* <fenv.h> on the device: the archive member build.py compiles in place of musl's src/fenv/fenv.c (generic
 * dummies, built because musl's x86-64 version is assembly; they report success for every rounding mode and
 * never see a flag). musl's own fesetround, feholdexcept, feupdateenv, fegetexceptflag and fesetexceptflag
 * are built on these functions.
 *
 * The environment is the floating-point runtime's state (rt/softfloat/libgcc.c): one rounding mode and one
 * set of exception flags, which every floating-point operation reads and raises. It is reached through the
 * runtime's four `__bennett_fe*` functions, in x86's encodings. The member is pulled from the archive only
 * when the program names one of these functions, so a program that never does keeps its floating point free
 * of state (no routine writes memory, the mode is a constant).
 *
 * As on x86-64 with glibc, which the native program uses: fesetround sets the mode of SSE and of x87 alike;
 * fetestexcept reads the flags of both, and only the five standard ones (glibc masks x87's denormal-operand
 * flag, `__FE_DENORM`, out, as its FE_ALL_EXCEPT does not have it; musl's FE_ALL_EXCEPT does, so it is masked
 * here too); feraiseexcept raises only those five. Traps (an unmasked exception), x87 precision other than
 * extended, SSE's flush-to-zero and denormals-are-zero modes, and different modes for SSE and x87 are not
 * carried: an environment that asks for one is a named fault (`fesetenv`), never a success that changes
 * nothing. */
#include <fenv.h>
#include <features.h>

int __bennett_fegetround(void);
void __bennett_fesetround(int);
int __bennett_fegetexcept(void);
void __bennett_fesetexcept(int);

/* The flags carried: x86's five standard ones, glibc's FE_ALL_EXCEPT. */
#define CARRIED (FE_INVALID | FE_DIVBYZERO | FE_OVERFLOW | FE_UNDERFLOW | FE_INEXACT)

/* The x87 control word's fields: the exception masks, the precision control and the rounding control. */
#define X87_MASKS 0x3f
#define X87_EXTENDED 0x300
#define X87_ROUNDING 0xc00
/* MXCSR's: the exception masks, the rounding control, flush-to-zero and denormals-are-zero. */
#define SSE_MASKS 0x1f80
#define SSE_ROUNDING_SHIFT 3
#define SSE_FLUSH 0x8040

int feclearexcept(int mask)
{
    __bennett_fesetexcept(__bennett_fegetexcept() & ~mask);
    return 0;
}

int feraiseexcept(int mask)
{
    __bennett_fesetexcept(__bennett_fegetexcept() | (mask & CARRIED));
    return 0;
}

int fetestexcept(int mask)
{
    return __bennett_fegetexcept() & mask & CARRIED;
}

int fegetround(void)
{
    return __bennett_fegetround();
}

/* musl's fesetround checks the mode first. */
hidden int __fesetround(int mode)
{
    __bennett_fesetround(mode);
    return 0;
}

/* What `fnstenv` and `stmxcsr` store on x86-64 for the carried state, with every x87 register empty; the
 * flags are in the x87 status word (where they are natively depends on which unit raised them; fesetenv and
 * fetestexcept read both). */
int fegetenv(fenv_t *environment)
{
    int mode = __bennett_fegetround();
    *environment = (fenv_t){
        .__control_word = 0x37f | mode,
        .__status_word = __bennett_fegetexcept(),
        .__tags = 0xffff,
        .__mxcsr = SSE_MASKS | mode << SSE_ROUNDING_SHIFT,
    };
    return 0;
}

/* As glibc's: the masks, precision and rounding of the x87 control word, its status word's flags, and all of
 * MXCSR are taken from the environment. Only what is carried is accepted. */
int fesetenv(const fenv_t *environment)
{
    if (environment == FE_DFL_ENV) {
        __bennett_fesetround(FE_TONEAREST);
        __bennett_fesetexcept(0);
        return 0;
    }
    unsigned control = environment->__control_word, mxcsr = environment->__mxcsr;
    int mode = control & X87_ROUNDING;
    if ((control & X87_MASKS) != X87_MASKS || (control & X87_EXTENDED) != X87_EXTENDED ||
        (mxcsr & SSE_MASKS) != SSE_MASKS || (mxcsr & SSE_FLUSH) ||
        (int)(mxcsr >> SSE_ROUNDING_SHIFT & X87_ROUNDING) != mode)
        __builtin_trap(); /* a trap, x87 precision, flush to zero or two rounding modes: not carried */
    __bennett_fesetround(mode);
    __bennett_fesetexcept((environment->__status_word | mxcsr) & CARRIED);
    return 0;
}
