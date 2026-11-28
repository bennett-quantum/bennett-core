#include "BaselineOptions.h"

#include <llvm/ADT/StringRef.h>
#include <llvm/TargetParser/Triple.h>

#include <cstddef>

using namespace llvm;

namespace bennett {

namespace {

enum class Form {
  Exact,  // the argument is the spelling
  Joined, // the argument is the spelling and a value of the closed form `isName` (`-march=x86-64-v3`)
  Value,  // the argument is the spelling and the next argument is `value`: both are forwarded
  Triple, // the spelling and a target triple (`isTriple`): joined if the spelling ends in `=`, else the next
          // argument; forwarded as `--target=TRIPLE`
};

struct Entry {
  const char *spelling;
  Form form;
  const char *value; // Form::Value only
};

// Code-generation options only: each reads no file and defines no macro of its own choosing. Grown from measured
// programs; an option not here is withheld from the baseline (see the header). The x86 target features, `-O4`
// (clang's `-O3`) and `-pg` were measured on a unit that discards two calls of bq_width: each was refused before
// it was here, and plain clang keeps both calls (review r1 of the step, P3).
constexpr Entry table[] = {
    // Optimisation level.
    {"-O", Form::Exact, nullptr},
    {"-O0", Form::Exact, nullptr},
    {"-O1", Form::Exact, nullptr},
    {"-O2", Form::Exact, nullptr},
    {"-O3", Form::Exact, nullptr},
    {"-Os", Form::Exact, nullptr},
    {"-Oz", Form::Exact, nullptr},
    {"-Og", Form::Exact, nullptr},
    {"-O4", Form::Exact, nullptr},
    {"-Ofast", Form::Exact, nullptr},
    {"-Xclang", Form::Value, "-disable-O0-optnone"},
    {"-fno-discard-value-names", Form::Exact, nullptr},
    {"-fdiscard-value-names", Form::Exact, nullptr},
    {"-fvectorize", Form::Exact, nullptr},
    {"-fno-vectorize", Form::Exact, nullptr},
    {"-fslp-vectorize", Form::Exact, nullptr},
    {"-fno-slp-vectorize", Form::Exact, nullptr},
    // Target.
    {"-target", Form::Triple, nullptr},
    {"--target=", Form::Triple, nullptr},
    {"-march=", Form::Joined, nullptr},
    {"-mtune=", Form::Joined, nullptr},
    {"-mcpu=", Form::Joined, nullptr},
    {"-mcmodel=", Form::Joined, nullptr},
    {"-m32", Form::Exact, nullptr},
    {"-m64", Form::Exact, nullptr},
    {"-mno-red-zone", Form::Exact, nullptr},
    {"-mred-zone", Form::Exact, nullptr},
    {"-mspeculative-load-hardening", Form::Exact, nullptr},
    {"-mno-speculative-load-hardening", Form::Exact, nullptr},
    {"-mstackrealign", Form::Exact, nullptr},
    {"-mno-stackrealign", Form::Exact, nullptr},
    {"-mbranch-protection=", Form::Joined, nullptr},
    {"-maes", Form::Exact, nullptr},
    {"-mavx2", Form::Exact, nullptr},
    {"-mbmi2", Form::Exact, nullptr},
    {"-mfma", Form::Exact, nullptr},
    {"-mno-avx", Form::Exact, nullptr},
    {"-mpopcnt", Form::Exact, nullptr},
    {"-msse4.2", Form::Exact, nullptr},
    // Floating point.
    {"-ffast-math", Form::Exact, nullptr},
    {"-fno-fast-math", Form::Exact, nullptr},
    {"-ffp-model=", Form::Joined, nullptr},
    {"-ffp-contract=", Form::Joined, nullptr},
    {"-fdenormal-fp-math=", Form::Joined, nullptr},
    {"-fmath-errno", Form::Exact, nullptr},
    {"-fno-math-errno", Form::Exact, nullptr},
    {"-frounding-math", Form::Exact, nullptr},
    {"-fno-rounding-math", Form::Exact, nullptr},
    // Library functions and the environment the code runs in.
    {"-fbuiltin", Form::Exact, nullptr},
    {"-fno-builtin", Form::Exact, nullptr},
    {"-fno-builtin-", Form::Joined, nullptr},
    {"-ffreestanding", Form::Exact, nullptr},
    {"-fhosted", Form::Exact, nullptr},
    {"-fexceptions", Form::Exact, nullptr},
    {"-fno-exceptions", Form::Exact, nullptr},
    {"-funwind-tables", Form::Exact, nullptr},
    {"-fno-unwind-tables", Form::Exact, nullptr},
    {"-fasynchronous-unwind-tables", Form::Exact, nullptr},
    {"-fno-asynchronous-unwind-tables", Form::Exact, nullptr},
    // Types.
    {"-funsigned-char", Form::Exact, nullptr},
    {"-fno-unsigned-char", Form::Exact, nullptr},
    {"-fsigned-char", Form::Exact, nullptr},
    {"-fno-signed-char", Form::Exact, nullptr},
    // Position independence and symbols.
    {"-fPIC", Form::Exact, nullptr},
    {"-fpic", Form::Exact, nullptr},
    {"-fPIE", Form::Exact, nullptr},
    {"-fpie", Form::Exact, nullptr},
    {"-fno-PIC", Form::Exact, nullptr},
    {"-fno-pic", Form::Exact, nullptr},
    {"-fno-PIE", Form::Exact, nullptr},
    {"-fno-pie", Form::Exact, nullptr},
    {"-fplt", Form::Exact, nullptr},
    {"-fno-plt", Form::Exact, nullptr},
    {"-fvisibility=", Form::Joined, nullptr},
    {"-ffunction-sections", Form::Exact, nullptr},
    {"-fdata-sections", Form::Exact, nullptr},
    {"-fcommon", Form::Exact, nullptr},
    {"-fno-common", Form::Exact, nullptr},
    // Stack and hardening.
    {"-fsplit-stack", Form::Exact, nullptr},
    {"-fstack-protector", Form::Exact, nullptr},
    {"-fstack-protector-all", Form::Exact, nullptr},
    {"-fstack-protector-strong", Form::Exact, nullptr},
    {"-fno-stack-protector", Form::Exact, nullptr},
    {"-fzero-call-used-regs=", Form::Joined, nullptr},
    {"-fcf-protection=", Form::Joined, nullptr},
    {"-fsanitize=", Form::Joined, nullptr},
    {"-fno-sanitize=", Form::Joined, nullptr},
    {"-fomit-frame-pointer", Form::Exact, nullptr},
    {"-fno-omit-frame-pointer", Form::Exact, nullptr},
    {"-pg", Form::Exact, nullptr},
    // Debug information (the check leaves `!dbg` out of every comparison).
    {"-g", Form::Exact, nullptr},
    {"-g0", Form::Exact, nullptr},
    {"-g1", Form::Exact, nullptr},
    {"-g2", Form::Exact, nullptr},
    {"-g3", Form::Exact, nullptr},
    {"-ggdb", Form::Exact, nullptr},
    {"-gline-tables-only", Form::Exact, nullptr},
    {"-gdwarf-", Form::Joined, nullptr},
};

// The options clang's driver reads the next arguments of as their values (SeparateOptions.def): how many it takes
// for `arg`, or 0.
unsigned separateValues(StringRef arg) {
  enum Match { Exact, Prefix };
  struct Separate {
    const char *spelling;
    Match match;
    unsigned values;
  };
  static constexpr Separate separate[] = {
#define BENNETT_SEPARATE(spelling, match, values) {spelling, match, values},
#include "SeparateOptions.def"
#undef BENNETT_SEPARATE
  };
  for (const Separate &option : separate) {
    if (option.match == Exact ? arg == option.spelling : arg.starts_with(option.spelling)) {
      return option.values;
    }
  }
  return 0;
}

// The closed form of a forwarded value: letters, digits and `_ . , + -`, the spelling of the names, numbers and
// lists the table's options take. It has no `@`, `/`, `=`, space or quote, so it is no response file and names no
// file.
bool isName(StringRef value) {
  return !value.empty() && value.find_first_not_of("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
                                                   "0123456789_.,+-") == StringRef::npos;
}

// A target triple: a name whose architecture LLVM knows.
bool isTriple(StringRef value) { return isName(value) && Triple(value).getArch() != Triple::UnknownArch; }

// The arguments `options[i]` and after it that the entry matches, forwarded as `forwarded`; 0 if it does not match.
std::size_t matches(const Entry &entry, const std::vector<std::string> &options, std::size_t i,
                    std::vector<std::string> &forwarded) {
  StringRef arg = options[i];
  bool hasNext = i + 1 < options.size();
  switch (entry.form) {
  case Form::Exact:
    if (arg == entry.spelling) {
      forwarded = {arg.str()};
      return 1;
    }
    return 0;
  case Form::Joined:
    if (arg.starts_with(entry.spelling) && isName(arg.drop_front(StringRef(entry.spelling).size()))) {
      forwarded = {arg.str()};
      return 1;
    }
    return 0;
  case Form::Value:
    if (arg == entry.spelling && hasNext && options[i + 1] == entry.value) {
      forwarded = {arg.str(), options[i + 1]};
      return 2;
    }
    return 0;
  case Form::Triple:
    if (StringRef(entry.spelling).ends_with("=")) {
      if (arg.starts_with(entry.spelling) && isTriple(arg.drop_front(StringRef(entry.spelling).size()))) {
        forwarded = {arg.str()};
        return 1;
      }
    } else if (arg == entry.spelling && hasNext && isTriple(options[i + 1])) {
      forwarded = {"--target=" + options[i + 1]};
      return 2;
    }
    return 0;
  }
  return 0;
}

} // namespace

BaselineOptions baselineOptions(const std::vector<std::string> &options) {
  BaselineOptions split;
  for (std::size_t i = 0; i < options.size();) {
    std::size_t used = 0;
    std::vector<std::string> forwarded;
    for (const Entry &entry : table) {
      if ((used = matches(entry, options, i, forwarded))) {
        break;
      }
    }
    if (used) {
      split.forwarded.insert(split.forwarded.end(), forwarded.begin(), forwarded.end());
      i += used;
      continue;
    }
    // Withheld, with the values clang reads after it, which are never read as options here (`-Xclang -O2`), and
    // after `--` everything: clang reads it as input files.
    std::size_t last = options[i] == "--" ? options.size() - 1 : i + separateValues(options[i]);
    for (; i <= last && i < options.size(); ++i) {
      split.withheld.push_back(options[i]);
    }
  }
  return split;
}

std::string baselineNote(const BaselineOptions &split) {
  std::string withheld;
  for (const std::string &arg : split.withheld) {
    withheld += (withheld.empty() ? "" : " ") + arg;
  }
  return " (the declaration check's baseline gets only the code-generation options of bennett-cc's table, an "
         "environment of only PATH, TMPDIR and CLANG_NO_DEFAULT_CONFIG=1, and no configuration file, so an option "
         "outside the table, a configuration file or an environment variable may be the cause; withheld from the "
         "baseline here: " +
         (withheld.empty() ? std::string("none") : withheld) + ")";
}

} // namespace bennett
