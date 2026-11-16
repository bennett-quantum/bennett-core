/* Ordinary global updates, with no quantum markers. The test appends the device drivers after
 * lowering. The old lookup source is included without its coherent wrappers or native main. */
#define JC_ORDINARY_ONLY
#include "journal_content.c"

static unsigned log_words[256], log_size;
static int insert_keys[2048];
unsigned insert_values[2048];

/* A degree-four B-tree, including root/child splits and replacement of existing keys. */
struct insert_node { int n, leaf, key[7], child[8]; unsigned value[7]; };
static struct insert_node tree[256];
static int tree_root, tree_size;

static int new_node(int leaf) {
  int at = tree_size++;
  tree[at].leaf = leaf;
  return at;
}

static void split_child(int p, int i) {
  int y = tree[p].child[i], z = new_node(tree[y].leaf);
  tree[z].n = 3;
  for (int j = 0; j < 3; j++) {
    tree[z].key[j] = tree[y].key[j + 4];
    tree[z].value[j] = tree[y].value[j + 4];
  }
  if (!tree[y].leaf)
    for (int j = 0; j < 4; j++) tree[z].child[j] = tree[y].child[j + 4];
  tree[y].n = 3;
  for (int j = tree[p].n; j > i; j--) tree[p].child[j + 1] = tree[p].child[j];
  tree[p].child[i + 1] = z;
  for (int j = tree[p].n - 1; j >= i; j--) {
    tree[p].key[j + 1] = tree[p].key[j];
    tree[p].value[j + 1] = tree[p].value[j];
  }
  tree[p].key[i] = tree[y].key[3];
  tree[p].value[i] = tree[y].value[3];
  tree[p].n++;
}

__attribute__((noinline)) static void insert_nonfull(int at, int key, unsigned value) {
  int i = 0;
  while (i < tree[at].n && key > tree[at].key[i]) i++;
  if (i < tree[at].n && key == tree[at].key[i]) { tree[at].value[i] = value; return; }
  if (tree[at].leaf) {
    for (int j = tree[at].n; j > i; j--) {
      tree[at].key[j] = tree[at].key[j - 1];
      tree[at].value[j] = tree[at].value[j - 1];
    }
    tree[at].key[i] = key; tree[at].value[i] = value; tree[at].n++;
  } else {
    if (tree[tree[at].child[i]].n == 7) {
      split_child(at, i);
      if (key == tree[at].key[i]) { tree[at].value[i] = value; return; }
      if (key > tree[at].key[i]) i++;
    }
    insert_nonfull(tree[at].child[i], key, value);
  }
}

static void tree_insert(int key, unsigned value) {
  if (tree[tree_root].n == 7) {
    int old = tree_root;
    tree_root = new_node(0); tree[tree_root].child[0] = old;
    split_child(tree_root, 0);
  }
  insert_nonfull(tree_root, key, value);
}

static unsigned tree_find(int key) {
  int at = tree_root;
  for (;;) {
    int i = 0;
    while (i < tree[at].n && key > tree[at].key[i]) i++;
    if (i < tree[at].n && key == tree[at].key[i]) return tree[at].value[i];
    if (tree[at].leaf) return 0;
    at = tree[at].child[i];
  }
}

void jg_setup(void) {
  jc_build();
  for (int i = 0; i < 2048; i++) insert_keys[i] = -1;
  for (int i = 0; i < 1024; i++) {
    unsigned at = jc_hash(4 * i);
    while (insert_keys[at] != -1) at = (at + 1) & 2047;
    insert_keys[at] = 4 * i;
    insert_values[at] = 37u * i + 11;
  }
  tree_root = new_node(1);
  for (int i = 0; i < 256; i++) tree_insert(4 * i, 37u * i + 11);
}

unsigned jg_log(unsigned char x, unsigned reps) {
  unsigned acc = x + 1;
  for (unsigned r = 0; r < reps; r++) {
    log_words[log_size++] = acc;
    acc = acc * 31 + log_words[(x + r) % log_size];
  }
  return acc ^ log_size;
}

unsigned jg_hash_insert(unsigned char x, unsigned reps) {
  unsigned acc = x + 1;
  for (unsigned r = 0; r < reps; r++) {
    int key = (int)(((4u * x + acc) & 4095) | 3);
    unsigned at = jc_hash(key);
    while (insert_keys[at] != -1 && insert_keys[at] != key) at = (at + 1) & 2047;
    insert_keys[at] = key;
    insert_values[at] = acc;
    acc = acc * 31 + insert_values[at] + (unsigned)key;
  }
  return acc;
}

unsigned jg_btree_insert(unsigned char x, unsigned reps) {
  unsigned acc = x + 1;
  for (unsigned r = 0; r < reps; r++) {
    int key = (int)((4u * x + acc) & 1023);
    tree_insert(key, acc);
    acc = acc * 31 + tree_find(key);
  }
  return acc;
}

#ifdef JG_NATIVE
#include <stdio.h>
#include <stdlib.h>
int main(int argc, char **argv) {
  if (argc != 4) return 1;
  jg_setup();
  unsigned x = (unsigned)atoi(argv[2]), reps = (unsigned)atoi(argv[3]);
  unsigned result;
  switch (atoi(argv[1])) {
  case 0: result = jg_log((unsigned char)x, reps); break;
  case 1: result = jg_hash_insert((unsigned char)x, reps); break;
  case 2: result = jg_btree_insert((unsigned char)x, reps); break;
  case 3: result = jc_binary_search((unsigned char)x, reps); break;
  case 4: result = jc_hash_lookup((unsigned char)x, reps); break;
  case 5: result = jc_btree_lookup((unsigned char)x, reps); break;
  default: result = jc_write_lookup((unsigned char)x, reps); break;
  }
  printf("%u\n", result);
  return 0;
}
#endif
