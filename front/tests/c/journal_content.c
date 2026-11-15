/* journal_content.c: memory-heavy lookups over 1,024 keys, for the journal-content measurement
 * (experiments/journal-content). The lookup functions are ordinary C and know nothing about qubits.
 * Keys are the 1,024 sorted values key(i) = 4i + (i mod 3), each with value(i) = (37 key(i) + 11)
 * mod 1000. jc_build() fills, once per run and before any qubit exists, a sorted array, an
 * open-addressing hash table of 2,048 slots, and a B-tree (at most 7 keys per node, separators are the
 * first key of the next child, as in a B+ tree) over the same key set.
 *
 * Each lookup takes an 8-bit branch value x and a repetition count: it looks up the key at index 4x,
 * folds the found value into an accumulator, and lets the accumulator choose the next index, so
 * branches diverge further as repetitions grow. jc_write_lookup is the hash lookup that also writes
 * memory, in arrays on its own stack: it counts visits per group of hash slots, and inserts each
 * found value into a sorted array of the values found so far (shifting larger ones up, so stores
 * overwrite values that differ between branches), and reads both back. (A coherent call to a
 * function that writes memory outside its own frame is refused by the front today, so the written
 * memory is the callee's stack.)
 *
 * With -DNATIVE it prints, for every x and each repetition count 1, 2, 4, 8, the four lookups in the
 * order binary search, hash, B-tree, write. Otherwise the coherent wrappers at the end put x in uniform
 * superposition over its 256 values (the coherent.c pattern): just before the first measurement every
 * branch holds (q, out) with out the lookup of its x. */

#define JC_NKEYS 1024
#define JC_HSIZE 2048
#define JC_BTKEYS 7
#define JC_BTCHILD 8
#define JC_BTNODES 256
#define JC_WGROUPS 16
#define JC_WRECENT 16

static int jc_sorted[JC_NKEYS];
static int jc_values[JC_NKEYS];
static int jc_hash_keys[JC_HSIZE];
static int jc_hash_values[JC_HSIZE];

struct jc_node {
  int n;
  int leaf;
  int keys[JC_BTKEYS];
  int vals[JC_BTKEYS];
  int child[JC_BTCHILD];
};
static struct jc_node jc_nodes[JC_BTNODES];
static int jc_root;
static int jc_built;

static unsigned jc_hash(int key) { return ((unsigned)key * 2654435761u >> 21) & (JC_HSIZE - 1); }

/* The smallest key under node `at` (a node's own keys[0] is that only for a leaf). */
static int jc_first_key(int at) {
  while (!jc_nodes[at].leaf)
    at = jc_nodes[at].child[0];
  return jc_nodes[at].keys[0];
}

__attribute__((noinline)) static void jc_build(void) {
  if (jc_built)
    return;
  for (int i = 0; i < JC_NKEYS; i++) {
    jc_sorted[i] = 4 * i + i % 3;
    jc_values[i] = (37 * jc_sorted[i] + 11) % 1000;
  }
  for (int i = 0; i < JC_HSIZE; i++)
    jc_hash_keys[i] = -1;
  for (int i = 0; i < JC_NKEYS; i++) {
    unsigned slot = jc_hash(jc_sorted[i]);
    while (jc_hash_keys[slot] != -1)
      slot = (slot + 1) & (JC_HSIZE - 1);
    jc_hash_keys[slot] = jc_sorted[i];
    jc_hash_values[slot] = jc_values[i];
  }
  int nnodes = 0;
  int level[JC_BTNODES];
  int nlevel = 0;
  for (int first = 0; first < JC_NKEYS; first += JC_BTKEYS) {
    struct jc_node *node = &jc_nodes[nnodes];
    int take = JC_NKEYS - first < JC_BTKEYS ? JC_NKEYS - first : JC_BTKEYS;
    node->n = take;
    node->leaf = 1;
    for (int i = 0; i < take; i++) {
      node->keys[i] = jc_sorted[first + i];
      node->vals[i] = jc_values[first + i];
    }
    for (int i = 0; i < JC_BTCHILD; i++)
      node->child[i] = -1;
    level[nlevel++] = nnodes++;
  }
  while (nlevel > 1) {
    int next[JC_BTNODES];
    int nnext = 0;
    for (int first = 0; first < nlevel; first += JC_BTCHILD) {
      struct jc_node *node = &jc_nodes[nnodes];
      int take = nlevel - first < JC_BTCHILD ? nlevel - first : JC_BTCHILD;
      node->n = take - 1;
      node->leaf = 0;
      for (int i = 0; i < take; i++) {
        node->child[i] = level[first + i];
        if (i > 0)
          node->keys[i - 1] = jc_first_key(level[first + i]);
      }
      next[nnext++] = nnodes++;
    }
    for (int i = 0; i < nnext; i++)
      level[i] = next[i];
    nlevel = nnext;
  }
  jc_root = level[0];
  jc_built = 1;
}

static int jc_hash_slot(int target) {
  unsigned slot = jc_hash(target);
  while (jc_hash_keys[slot] != target)
    slot = (slot + 1) & (JC_HSIZE - 1);
  return (int)slot;
}

__attribute__((noinline)) unsigned jc_binary_search(unsigned char x, unsigned reps) {
  unsigned idx = 4u * x;
  unsigned acc = 0;
  for (unsigned r = 0; r < reps; r++) {
    int target = jc_sorted[idx];
    int lo = 0, hi = JC_NKEYS - 1, pos = -1;
    while (lo <= hi) {
      int mid = (lo + hi) >> 1;
      if (jc_sorted[mid] == target) {
        pos = mid;
        break;
      }
      if (jc_sorted[mid] < target)
        lo = mid + 1;
      else
        hi = mid - 1;
    }
    acc = acc * 31 + (unsigned)jc_values[pos];
    idx = ((unsigned)pos + acc) & (JC_NKEYS - 1);
  }
  return acc;
}

__attribute__((noinline)) unsigned jc_hash_lookup(unsigned char x, unsigned reps) {
  unsigned idx = 4u * x;
  unsigned acc = 0;
  for (unsigned r = 0; r < reps; r++) {
    int slot = jc_hash_slot(jc_sorted[idx]);
    acc = acc * 31 + (unsigned)jc_hash_values[slot];
    idx = (idx + acc) & (JC_NKEYS - 1);
  }
  return acc;
}

__attribute__((noinline)) unsigned jc_btree_lookup(unsigned char x, unsigned reps) {
  unsigned idx = 4u * x;
  unsigned acc = 0;
  for (unsigned r = 0; r < reps; r++) {
    int target = jc_sorted[idx];
    int at = jc_root;
    while (!jc_nodes[at].leaf) {
      int i = 0;
      while (i < jc_nodes[at].n && target >= jc_nodes[at].keys[i])
        i++;
      at = jc_nodes[at].child[i];
    }
    int value = -1;
    for (int i = 0; i < jc_nodes[at].n; i++)
      if (jc_nodes[at].keys[i] == target)
        value = jc_nodes[at].vals[i];
    acc = acc * 31 + (unsigned)value;
    idx = (idx + acc) & (JC_NKEYS - 1);
  }
  return acc;
}

__attribute__((noinline)) unsigned jc_write_lookup(unsigned char x, unsigned reps) {
  int counts[JC_WGROUPS];
  int recent[JC_WRECENT];
  for (int i = 0; i < JC_WGROUPS; i++)
    counts[i] = 0;
  int nrecent = 0;
  unsigned idx = 4u * x;
  unsigned acc = 0;
  for (unsigned r = 0; r < reps; r++) {
    int slot = jc_hash_slot(jc_sorted[idx]);
    int value = jc_hash_values[slot];
    int group = slot & (JC_WGROUPS - 1);
    counts[group] += 1;
    int at = nrecent < JC_WRECENT ? nrecent++ : JC_WRECENT - 1;
    while (at > 0 && recent[at - 1] > value) {
      recent[at] = recent[at - 1];
      at--;
    }
    recent[at] = value;
    acc = acc * 31 + (unsigned)value + (unsigned)counts[group] + (unsigned)recent[nrecent / 2];
    idx = (idx + acc) & (JC_NKEYS - 1);
  }
  return acc;
}

#ifdef NATIVE
#include <stdio.h>

int main(void) {
  jc_build();
  for (int x = 0; x < 256; x++) {
    for (unsigned reps = 1; reps <= 8; reps *= 2) {
      unsigned b = jc_binary_search((unsigned char)x, reps);
      unsigned h = jc_hash_lookup((unsigned char)x, reps);
      unsigned t = jc_btree_lookup((unsigned char)x, reps);
      unsigned w = jc_write_lookup((unsigned char)x, reps);
      printf("%d %u %u %u %u %u\n", x, reps, b, h, t, w);
    }
  }
  return 0;
}

#else
#include "bennett.h"

#define COHERENT(f)                                                                                          \
  unsigned long long coherent_##f(unsigned char key, unsigned reps) {                                        \
    jc_build();                                                                                              \
    _Bool q[8] = {0};                                                                                        \
    for (int i = 0; i < 8; i++) bennett_h(&q[i]);                                                            \
    unsigned char x = (unsigned char)(key * 37 + 11);                                                        \
    for (int i = 0; i < 8; i++) x ^= (unsigned char)(q[i] << i);                                             \
    unsigned out = f(x, reps);                                                                               \
    unsigned long long y = bennett_measure(&out);                                                            \
    unsigned long long bits = bennett_measure(q);                                                            \
    unsigned long long measured = 0;                                                                         \
    for (int i = 0; i < 8; i++) measured |= (bits >> (8 * i) & 1) << i;                                     \
    return (measured ^ (unsigned char)(key * 37 + 11)) << 32 | (y & 0xffffffffu);                            \
  }

COHERENT(jc_binary_search)
COHERENT(jc_hash_lookup)
COHERENT(jc_btree_lookup)
COHERENT(jc_write_lookup)

#endif
