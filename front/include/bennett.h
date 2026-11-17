/* bennett.h: the marker calls of the Bennett toolchain. See docs/MARKERS.md.
 *
 * Every marker is an ordinary external function. It is never defined: `bennett-front` recognises the calls
 * by name and signature. A marker may read and write memory and is not declared pure, so stock LLVM keeps
 * every call, in order, on every path that executes it.
 *
 * A qubit variable is a local scalar variable (at most 8 bytes) whose address is given to a marker. It
 * starts at 0. Gates act on one `_Bool` of it: the byte the pointer names, bit 0.
 */
#ifndef BENNETT_H
#define BENNETT_H

#ifdef __cplusplus
extern "C" {
#define BENNETT_BOOL bool
#else
#define BENNETT_BOOL _Bool
#endif

void bennett_x(BENNETT_BOOL *qubit);
void bennett_h(BENNETT_BOOL *qubit);
void bennett_z(BENNETT_BOOL *qubit);
void bennett_s(BENNETT_BOOL *qubit);
void bennett_sdg(BENNETT_BOOL *qubit);
void bennett_t(BENNETT_BOOL *qubit);
void bennett_tdg(BENNETT_BOOL *qubit);
void bennett_rx(BENNETT_BOOL *qubit, double theta);
void bennett_ry(BENNETT_BOOL *qubit, double theta);
void bennett_rz(BENNETT_BOOL *qubit, double theta);

/* The qubit of bit `bit` (a constant) of the qubit variable `variable` points to, for the gates above:
 * bennett_h(bennett_bit(&x, 3)) is a Hadamard on bit 3 of x. Its result may only be given to a gate. */
BENNETT_BOOL *bennett_bit(void *variable, unsigned bit);

/* Measures the whole qubit variable `variable` points to and returns its value, zero-extended. */
unsigned long long bennett_measure(const void *variable);
/* Measures the `bytes` bytes at `address` (a constant from 1 to 8), little-endian, and returns their value,
 * zero-extended: the bits of bytes that hold qubits are measured, the others read. Afterwards the bytes are
 * ordinary memory holding the result. The measurement a program writes before ordinary code reads memory
 * that was written under a qubit. */
unsigned long long bennett_measure_bytes(const void *address, unsigned bytes);

/* Device console byte output. An ordinary effectful call, not a quantum marker. */
void __bennett_effect(char byte);

/* Finish the whole run with an int status, even in a nested call. */
#ifdef __cplusplus
[[noreturn]] void __bennett_exit(int status);
#else
_Noreturn void __bennett_exit(int status);
#endif

#ifdef __cplusplus
}
#endif

#undef BENNETT_BOOL
#endif
