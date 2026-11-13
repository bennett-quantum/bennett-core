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
#endif

void bennett_x(_Bool *qubit);
void bennett_h(_Bool *qubit);
void bennett_z(_Bool *qubit);
void bennett_s(_Bool *qubit);
void bennett_sdg(_Bool *qubit);
void bennett_t(_Bool *qubit);
void bennett_tdg(_Bool *qubit);
void bennett_rx(_Bool *qubit, double theta);
void bennett_ry(_Bool *qubit, double theta);
void bennett_rz(_Bool *qubit, double theta);

/* Measures the whole qubit variable `variable` points to and returns its value, zero-extended. */
unsigned long long bennett_measure(const void *variable);

#ifdef __cplusplus
}
#endif

#endif
