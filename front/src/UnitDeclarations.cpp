#include "UnitDeclarations.h"

#include "Diagnostic.h"
#include "Primitives.h"

#include <llvm/ADT/ArrayRef.h>
#include <llvm/ADT/SmallString.h>
#include <llvm/ADT/StringRef.h>
#include <llvm/ADT/StringSet.h>
#include <llvm/Support/FileSystem.h>
#include <llvm/Support/JSON.h>

#include <string>
#include <vector>

using namespace llvm;

namespace bennett {

namespace {

constexpr const char *route = "; the header bq.h, included by force in every unit, has the only declaration of each "
                              "primitive (docs/HANDLES.md, section 4)";

// Splits JSON text into values, so that each is parsed alone (the parse itself is LLVM's). With `within` 0, the
// values one after another at the top (clang writes one object per declaration a filter matches); with `within` 2,
// the elements of the array in the top object (the unfiltered dump is one TranslationUnitDecl whose only array,
// "inner", holds the unit's top-level declarations), so that no value is larger than one declaration.
std::vector<StringRef> values(StringRef text, size_t within) {
  std::vector<StringRef> found;
  std::vector<char> open; // the brackets open at this point
  size_t start = 0;
  bool inString = false;
  for (size_t i = 0; i < text.size(); ++i) {
    char c = text[i];
    if (inString) {
      if (c == '\\') {
        ++i;
      } else if (c == '"') {
        inString = false;
      }
      continue;
    }
    if (c == '"') {
      inString = true;
    } else if (c == '{' || c == '[') {
      if (open.size() == within && (within == 0 || open.back() == '[')) {
        start = i;
      }
      open.push_back(c);
    } else if (c == '}' || c == ']') {
      if (open.empty() || open.back() != (c == '}' ? '{' : '[')) {
        fail("the declaration dump is not clang's JSON: an unmatched closing bracket");
      }
      open.pop_back();
      if (open.size() == within && (within == 0 || open.back() == '[')) {
        found.push_back(text.slice(start, i + 1));
      }
    }
  }
  if (!open.empty() || inString) {
    fail("the declaration dump is not clang's JSON: it ends inside a value");
  }
  return found;
}

std::string canonical(StringRef path) {
  SmallString<256> real;
  if (sys::fs::real_path(path, real)) {
    return path.str();
  }
  return std::string(real);
}

// Where a declaration is: its file, line and column as far as the dump says; clang leaves out a part that is
// the same as in the location it wrote before, so the parts are carried down from the enclosing declaration.
struct Where {
  std::string file;
  int64_t line = 0;
  int64_t column = 0;

  std::string text() const {
    return (file.empty() ? std::string("an unknown file") : file) + ":" + std::to_string(line) + ":" +
           std::to_string(column);
  }
};

Where locate(const json::Object &declaration, Where outer) {
  const json::Object *loc = declaration.getObject("loc");
  if (!loc) {
    return outer;
  }
  if (const json::Object *expansion = loc->getObject("expansionLoc")) {
    loc = expansion; // a declaration written by a macro: where the macro is used
  }
  if (std::optional<StringRef> file = loc->getString("file")) {
    outer.file = file->str();
  }
  if (std::optional<int64_t> line = loc->getInteger("line")) {
    outer.line = *line;
  }
  if (std::optional<int64_t> column = loc->getInteger("col")) {
    outer.column = *column;
  }
  return outer;
}

bool defines(const json::Object &declaration) {
  if (const json::Array *inner = declaration.getArray("inner")) {
    for (const json::Value &child : *inner) {
      const json::Object *object = child.getAsObject();
      std::optional<StringRef> kind = object ? object->getString("kind") : std::nullopt;
      if (kind && (*kind == "CompoundStmt" || *kind == "AliasAttr" || *kind == "IFuncAttr")) {
        return true;
      }
    }
  }
  return declaration.getString("kind") == StringRef("VarDecl") && declaration.get("init") != nullptr;
}

// The attributes of a declaration's final form, which clang lists among its children (an attribute added by a
// later pragma or redeclaration is there too, marked implicit or inherited).
std::vector<StringRef> attributes(const json::Object &declaration) {
  std::vector<StringRef> found;
  if (const json::Array *inner = declaration.getArray("inner")) {
    for (const json::Value &child : *inner) {
      const json::Object *object = child.getAsObject();
      std::optional<StringRef> kind = object ? object->getString("kind") : std::nullopt;
      if (kind && kind->ends_with("Attr")) {
        found.push_back(*kind);
      }
    }
  }
  return found;
}

struct Checker {
  std::string header;
  // In the unfiltered dump, where clang writes a file name only when it changes and the order of keys is lost in
  // the parse, a declaration's file is not known: only clause 2, which needs none, is checked there.
  bool full = false;
  StringSet<> declared; // primitives the header declares in this unit

  void visit(const json::Value &value, const Where &outer, bool topLevel) {
    if (const json::Array *array = value.getAsArray()) {
      for (const json::Value &child : *array) {
        visit(child, outer, false);
      }
      return;
    }
    const json::Object *object = value.getAsObject();
    if (!object) {
      return;
    }
    Where where = locate(*object, outer);
    std::optional<StringRef> kind = object->getString("kind");
    if (kind && (*kind == "FunctionDecl" || *kind == "VarDecl")) {
      declaration(*object, *kind, where, topLevel);
    }
    if (const json::Value *inner = object->get("inner")) {
      visit(*inner, where, false);
    }
  }

  void declaration(const json::Object &object, StringRef kind, const Where &where, bool topLevel) {
    StringRef name = object.getString("name").value_or("");
    std::optional<StringRef> mangled = object.getString("mangledName");
    if (!mangled) {
      return; // no linkage: a local variable or a parameter, which only hides the name
    }
    StringRef linker = mangled->starts_with("\1") ? mangled->drop_front() : *mangled;
    std::optional<Primitive> byName = primitiveNamed(name);
    std::optional<Primitive> byLinker = primitiveNamed(linker);
    if (!byName && !byLinker) {
      return;
    }
    if (full) {
      if (!byName) {
        fail("the declaration of " + name.str() + " has the linker name of the primitive " + linker.str() +
             " (an assembler label or #pragma redefine_extname)" + route);
      }
      return; // a declaration of the primitive's identifier: the filtered dumps show where it is
    }
    bool fromHeader = topLevel && kind == "FunctionDecl" && byName && !object.get("previousDecl") &&
                      !where.file.empty() && canonical(where.file) == header;
    if (fromHeader) {
      declared.insert(name);
      if (*mangled != name) {
        fail("the header's declaration of the primitive " + name.str() + " has the linker name `" + linker.str() +
             "` at the end of the unit (a #pragma redefine_extname or an assembler label renames it); calls of " +
             name.str() + " would not reach the primitive" + route + ", at " + where.text());
      }
      for (StringRef attribute : attributes(object)) {
        if (!(attribute == "C11NoReturnAttr" && byName->noreturn)) {
          fail("the header's declaration of the primitive " + name.str() + " has the attribute " + attribute.str() +
               " at the end of the unit, which the header does not give it (a #pragma weak or another pragma adds "
               "it); the header's declarations keep their linkage and attributes" +
               route + ", at " + where.text());
        }
      }
      return;
    }
    if (byName) {
      fail(std::string(defines(object) ? "a definition" : "a declaration") + " of the primitive " + name.str() +
           " outside the handle header" + route + ", at " + where.text());
    }
    fail("the declaration of " + name.str() + " has the linker name of the primitive " + linker.str() +
         " (an assembler label or #pragma redefine_extname)" + route + ", at " + where.text());
  }
};

} // namespace

void checkUnitDeclarations(StringRef filtered, StringRef full, StringRef header) {
  Checker checker;
  checker.header = canonical(header);
  for (bool whole : {false, true}) {
    checker.full = whole;
    for (StringRef text : values(whole ? full : filtered, whole ? 2 : 0)) {
      Expected<json::Value> value = json::parse(text);
      if (!value) {
        fail("the declaration dump is not clang's JSON: " + toString(value.takeError()));
      }
      checker.visit(*value, Where(), true);
    }
  }
  for (const Primitive &primitive : allPrimitives()) {
    if (!checker.declared.contains(primitive.name)) {
      fail(std::string("the unit has no declaration of the primitive ") + primitive.name + " from the handle header " +
           header.str() + ": the driver includes it by force in every unit (clang -include; docs/HANDLES.md, "
           "section 4)");
    }
  }
}

} // namespace bennett
