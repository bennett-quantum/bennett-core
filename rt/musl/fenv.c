/* <fenv.h> on the device: build.py compiles this file in place of musl's src/fenv/fenv.c (generic dummies,
 * built because musl's x86-64 version is assembly; they report success for every rounding mode and never see a
 * flag), with -DM_fenv, and in place of musl's fesetexceptflag.c and feholdexcept.c, with -DM_fesetexceptflag
 * and -DM_feholdexcept. musl's own fesetround, feupdateenv and fegetexceptflag are built on these functions.
 *
 * The environment is the floating-point runtime's state (rt/softfloat/libgcc.c), as x86 has it: two units, SSE
 * (float and double, as clang's x86-64 code computes them) and x87 (long double), each with its own rounding
 * mode and its own six status flags (invalid 1, denormal operand 2, divide by zero 4, overflow 8, underflow 16,
 * inexact 32), reached through the runtime's `__bennett_fe*` functions. Each function does what glibc's does
 * on x86-64, which the native program uses: fesetround sets both units' modes, fegetround reads x87's;
 * fetestexcept reads the union of both units' flags; feclearexcept clears both (and x87's denormal-operand flag
 * always); feraiseexcept raises invalid
 * and divide by zero in SSE and overflow, underflow and inexact in x87; fesetexceptflag sets and clears SSE's
 * flags and only clears x87's. These
 * take only the five standard flags (glibc's FE_ALL_EXCEPT; musl's also names the denormal-operand flag, which
 * glibc masks out). fegetenv and fesetenv carry both halves of fenv_t, the denormal-operand flag included.
 *
 * The member is pulled from the archive only when the program names one of these functions, so a program that
 * never does keeps its floating point free of state (no routine writes memory, the modes are constants).
 *
 * Not carried, and a named fault in fesetenv (the device's fault `unreachable`: the device's faults are a
 * fixed table with no room for a reason): an unmasked exception (a trap), x87 precision other than extended,
 * SSE's flush-to-zero and denormals-are-zero modes, and a reserved MXCSR bit, which faults natively too. */
#include <fenv.h>
#include <features.h>

int __bennett_fegetround(void);
void __bennett_fesetround(int);
int __bennett_fegetexcept(void);
void __bennett_fesetexcept(int);
int __bennett_fegetunit(int);
void __bennett_fesetunit(int, int);

enum { SSE = 0, X87 = 1 };

/* The flags carried by the C interface: x86's five standard ones, glibc's FE_ALL_EXCEPT. */
#define CARRIED (FE_INVALID | FE_DIVBYZERO | FE_OVERFLOW | FE_UNDERFLOW | FE_INEXACT)
/* The six status flags of each unit. */
#define STATUS 0x3f
/* The rounding control, as each unit's state word from __bennett_fegetunit has it (x87's RC field). */
#define ROUNDING 0xc00

/* The x87 control word's fields: the exception masks, the precision control. */
#define X87_MASKS 0x3f
#define X87_EXTENDED 0x300
/* MXCSR's: the exception masks, the rounding control (at bit 13), flush-to-zero and denormals-are-zero; its
 * other bits above the flags are reserved. */
#define SSE_MASKS 0x1f80
#define SSE_ROUNDING_SHIFT 3
#define SSE_FLUSH 0x8040
#define SSE_DEFINED 0xffff

#ifdef M_fenv
/* As glibc's: SSE's flags in `mask` are cleared; x87's status word keeps only the standard flags not in `mask`
 * (glibc ands it with `mask ^ FE_ALL_EXCEPT`), so its denormal-operand flag is cleared on every call. */
int feclearexcept(int mask)
{
    mask &= CARRIED;
    __bennett_fesetunit(SSE, __bennett_fegetunit(SSE) & ~mask);
    __bennett_fesetunit(X87, __bennett_fegetunit(X87) & ~(STATUS & ~(CARRIED & ~mask)));
    return 0;
}

/* As glibc's: invalid (0/0) and divide by zero (1/0) by SSE divisions, the others in the x87 status word. */
int feraiseexcept(int mask)
{
    int sse = mask & (FE_INVALID | FE_DIVBYZERO), x87 = mask & (FE_OVERFLOW | FE_UNDERFLOW | FE_INEXACT);
    __bennett_fesetunit(SSE, __bennett_fegetunit(SSE) | sse);
    __bennett_fesetunit(X87, __bennett_fegetunit(X87) | x87);
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

/* What `fnstenv` and `stmxcsr` store on x86-64 for the carried state, with every x87 register empty. */
int fegetenv(fenv_t *environment)
{
    int sse = __bennett_fegetunit(SSE), x87 = __bennett_fegetunit(X87);
    *environment = (fenv_t){
        .__control_word = 0x37f | (x87 & ROUNDING),
        .__status_word = x87 & STATUS,
        .__tags = 0xffff,
        .__mxcsr = SSE_MASKS | (sse & ROUNDING) << SSE_ROUNDING_SHIFT | (sse & STATUS),
    };
    return 0;
}

/* As glibc's: the masks, precision and rounding of the x87 control word, its status word's six flags, and all
 * of MXCSR are taken from the environment. Only what is carried is accepted. */
int fesetenv(const fenv_t *environment)
{
    if (environment == FE_DFL_ENV) {
        __bennett_fesetround(FE_TONEAREST);
        __bennett_fesetexcept(0);
        return 0;
    }
    unsigned control = environment->__control_word, mxcsr = environment->__mxcsr;
    if ((control & X87_MASKS) != X87_MASKS || (control & X87_EXTENDED) != X87_EXTENDED)
        __builtin_trap(); /* an unmasked x87 exception or x87 precision: not carried */
    if ((mxcsr & ~SSE_DEFINED) || (mxcsr & SSE_MASKS) != SSE_MASKS || (mxcsr & SSE_FLUSH))
        __builtin_trap(); /* a reserved MXCSR bit, an unmasked SSE exception or flush to zero: not carried */
    __bennett_fesetunit(X87, (control & ROUNDING) | (environment->__status_word & STATUS));
    __bennett_fesetunit(SSE, (mxcsr >> SSE_ROUNDING_SHIFT & ROUNDING) | (mxcsr & STATUS));
    return 0;
}
#endif

#ifdef M_fesetexceptflag
/* As glibc's: SSE's flags in `mask` become those of the saved value; x87's in `mask` that the saved value does
 * not have are cleared, and none is set there. */
int fesetexceptflag(const fexcept_t *saved, int mask)
{
    mask &= CARRIED;
    int sse = __bennett_fegetunit(SSE), x87 = __bennett_fegetunit(X87);
    __bennett_fesetunit(SSE, sse ^ ((sse ^ *saved) & mask));
    __bennett_fesetunit(X87, x87 & ~(mask & ~*saved));
    return 0;
}
#endif

#ifdef M_feholdexcept
/* As glibc's: the environment saved, then every flag of both units cleared, the denormal-operand flag too. */
int feholdexcept(fenv_t *environment)
{
    fegetenv(environment);
    for (int unit = SSE; unit <= X87; ++unit)
        __bennett_fesetunit(unit, __bennett_fegetunit(unit) & ~STATUS);
    return 0;
}
#endif
