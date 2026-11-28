#include "BaselineOptions.h"

#include <llvm/ADT/StringRef.h>

#include <cstddef>

using namespace llvm;

namespace bennett {

namespace {

enum class Form {
  Exact,  // the argument is the spelling
  Joined, // the argument starts with the spelling (a value joined to it, `-O2`, `-march=x86-64`)
  Value,  // the argument is the spelling and the next argument is `value`: both are forwarded
  Next,   // the argument is the spelling and the next argument is its value, whatever it is
};

struct Entry {
  const char *spelling;
  Form form;
  const char *value; // Form::Value only
};

// Code-generation options only: each reads no file and defines no macro of its own choosing. Grown from measured
// programs; an option not here is withheld from the baseline (see the header).
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
    {"-Ofast", Form::Exact, nullptr},
    {"-Xclang", Form::Value, "-disable-O0-optnone"},
    {"-fno-discard-value-names", Form::Exact, nullptr},
    {"-fdiscard-value-names", Form::Exact, nullptr},
    {"-fvectorize", Form::Exact, nullptr},
    {"-fno-vectorize", Form::Exact, nullptr},
    {"-fslp-vectorize", Form::Exact, nullptr},
    {"-fno-slp-vectorize", Form::Exact, nullptr},
    // Target.
    {"-target", Form::Next, nullptr},
    {"--target=", Form::Joined, nullptr},
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

} // namespace

BaselineOptions baselineOptions(const std::vector<std::string> &options) {
  BaselineOptions split;
  for (std::size_t i = 0; i < options.size(); ++i) {
    StringRef arg = options[i];
    bool hasNext = i + 1 < options.size();
    bool matched = false;
    for (const Entry &entry : table) {
      switch (entry.form) {
      case Form::Exact:
        matched = arg == entry.spelling;
        break;
      case Form::Joined:
        matched = arg.starts_with(entry.spelling);
        break;
      case Form::Value:
        matched = arg == entry.spelling && hasNext && options[i + 1] == entry.value;
        break;
      case Form::Next:
        matched = arg == entry.spelling && hasNext;
        break;
      }
      if (matched) {
        split.forwarded.push_back(options[i]);
        if (entry.form == Form::Value || entry.form == Form::Next) {
          split.forwarded.push_back(options[++i]);
        }
        break;
      }
    }
    if (!matched) {
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
  return " (the baseline it is compared with gets only the code-generation options of bennett-cc's table, no "
         "environment and no configuration file, so an option outside the table, a configuration file or an "
         "environment variable may be the cause; withheld from the baseline here: " +
         (withheld.empty() ? std::string("none") : withheld) + ")";
}

} // namespace bennett
