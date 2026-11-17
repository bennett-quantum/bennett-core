# bennett-front: LLVM bitcode in, Bennett device program text out. The only part of the project that links
# LLVM. Configure with the LLVM 18 CMake package, for example
#   cmake -S front -B target/front -DLLVM_DIR="$(llvm-config-18 --cmakedir)"
cmake_minimum_required(VERSION 3.20)
project(bennett-front LANGUAGES CXX)

find_package(LLVM 18.1 REQUIRED CONFIG)
file(STRINGS "${CMAKE_CURRENT_SOURCE_DIR}/llvm-major.txt" BENNETT_LLVM_MAJOR)
if(NOT LLVM_VERSION_MAJOR EQUAL BENNETT_LLVM_MAJOR)
  message(FATAL_ERROR "Bennett requires pinned LLVM major ${BENNETT_LLVM_MAJOR}, found ${LLVM_PACKAGE_VERSION}")
endif()

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release)
endif()

add_executable(bennett-front
  src/main.cpp
  src/Abi.cpp
  src/Diagnostic.cpp
  src/Markers.cpp
  src/Normalise.cpp
  src/Taint.cpp
  src/Outline.cpp
  src/Emit.cpp
  src/Control.cpp
  src/Predicate.cpp
  src/InPlace.cpp
  src/Startup.cpp
  src/SoftFloat.cpp
  src/Denormal.cpp
  src/FpClass.cpp
  src/Complex.cpp
  src/Jumps.cpp
)
target_include_directories(bennett-front SYSTEM PRIVATE ${LLVM_INCLUDE_DIRS})
target_compile_definitions(bennett-front PRIVATE ${LLVM_DEFINITIONS})
target_compile_options(bennett-front PRIVATE -Wall -Wextra -fno-rtti -fno-exceptions)
if(LLVM_LINK_LLVM_DYLIB)
  target_link_libraries(bennett-front PRIVATE LLVM)
else()
  llvm_map_components_to_libnames(llvm_libs core irreader bitreader passes transformsutils analysis codegen)
  target_link_libraries(bennett-front PRIVATE ${llvm_libs})
endif()

# The default floating-point runtime, `float-runtime.bc` next to bennett-front: Berkeley SoftFloat 3e
# (rt/softfloat, vendored unmodified) under the compiler runtime-call names of rt/softfloat/libgcc.c,
# compiled to bitcode by clang 18 and linked into one file. The SoftFloat files are those its own
# Linux-x86_64-GCC Makefile lists, with its options, except softfloat_raiseFlags.c: libgcc.c has its own.
# clang is $CLANG if set, as for the tests, else the one next to LLVM's tools, else on the path.
if(DEFINED ENV{CLANG})
  set(BENNETT_CLANG "$ENV{CLANG}")
else()
  find_program(BENNETT_CLANG NAMES clang clang-18 HINTS "${LLVM_TOOLS_BINARY_DIR}" REQUIRED)
endif()
find_program(BENNETT_LLVM_LINK NAMES llvm-link llvm-link-18 HINTS "${LLVM_TOOLS_BINARY_DIR}" REQUIRED)
foreach(tool BENNETT_CLANG BENNETT_LLVM_LINK)
  execute_process(COMMAND "${${tool}}" --version OUTPUT_VARIABLE tool_version RESULT_VARIABLE tool_status)
  if(NOT tool_status EQUAL 0 OR NOT tool_version MATCHES "version ${BENNETT_LLVM_MAJOR}\\.")
    message(FATAL_ERROR "${tool} is not pinned major ${BENNETT_LLVM_MAJOR}: ${tool_version}")
  endif()
endforeach()


set(softfloat "${CMAKE_CURRENT_SOURCE_DIR}/../rt/softfloat")
set(softfloat_source "${softfloat}/berkeley-softfloat-3e/source")
set(softfloat_build "${softfloat}/berkeley-softfloat-3e/build/Linux-x86_64-GCC")
file(READ "${softfloat_build}/Makefile" softfloat_makefile)
string(REGEX MATCHALL "\n  [A-Za-z0-9_]+\\$\\(OBJ\\)" softfloat_objects "${softfloat_makefile}")
set(softfloat_flags
  -O2 -fno-vectorize -fno-slp-vectorize -ffreestanding -fno-builtin -Werror-implicit-function-declaration
  -emit-llvm -c
  -DSOFTFLOAT_FAST_INT64 -DSOFTFLOAT_ROUND_ODD -DINLINE_LEVEL=5 -DSOFTFLOAT_FAST_DIV32TO16
  -DSOFTFLOAT_FAST_DIV64TO32
  -I "${softfloat_build}" -I "${softfloat_source}/8086-SSE" -I "${softfloat_source}/include")
set(softfloat_bitcode)
file(MAKE_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/softfloat")
foreach(line IN LISTS softfloat_objects)
  string(REGEX REPLACE "^\n  ([A-Za-z0-9_]+).*" "\\1" name "${line}")
  if(name STREQUAL "softfloat_raiseFlags")
    continue()
  endif()
  if(EXISTS "${softfloat_source}/8086-SSE/${name}.c")
    set(file "${softfloat_source}/8086-SSE/${name}.c")
  else()
    set(file "${softfloat_source}/${name}.c")
  endif()
  set(bitcode "${CMAKE_CURRENT_BINARY_DIR}/softfloat/${name}.bc")
  add_custom_command(OUTPUT "${bitcode}"
    COMMAND "${BENNETT_CLANG}" ${softfloat_flags} "${file}" -o "${bitcode}"
    DEPENDS "${file}" VERBATIM)
  list(APPEND softfloat_bitcode "${bitcode}")
endforeach()
set(interface_bitcode "${CMAKE_CURRENT_BINARY_DIR}/softfloat/libgcc.bc")
add_custom_command(OUTPUT "${interface_bitcode}"
  COMMAND "${BENNETT_CLANG}" ${softfloat_flags} "${softfloat}/libgcc.c" -o "${interface_bitcode}"
  DEPENDS "${softfloat}/libgcc.c" VERBATIM)
add_custom_command(OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/float-runtime.bc"
  COMMAND "${BENNETT_LLVM_LINK}" "${interface_bitcode}" ${softfloat_bitcode}
          -o "${CMAKE_CURRENT_BINARY_DIR}/float-runtime.bc"
  DEPENDS "${interface_bitcode}" ${softfloat_bitcode} VERBATIM)
add_custom_target(float-runtime ALL DEPENDS "${CMAKE_CURRENT_BINARY_DIR}/float-runtime.bc")

# The complex multiply and divide helpers, `complex-runtime.bc` next to bennett-front: GCC 13.3.0's own
# sources (rt/libgcc, vendored unmodified), compiled to bitcode with floating-point operations intact
# (the front links them into the program before lowering its floating point) and linked into one file.
set(libgcc "${CMAKE_CURRENT_SOURCE_DIR}/../rt/libgcc")
set(libgcc_source "${libgcc}/gcc-13.3.0")
file(GLOB_RECURSE libgcc_headers CONFIGURE_DEPENDS "${libgcc}/*.h")
set(complex_flags
  -O2 -fno-vectorize -fno-slp-vectorize -ffreestanding -fno-builtin -Werror-implicit-function-declaration
  -ffp-contract=off -emit-llvm -c
  -I "${libgcc}/config" -I "${libgcc_source}/gcc" -I "${libgcc_source}/include")
set(complex_bitcode)
file(MAKE_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/complex")
foreach(name mulsc3 divsc3 muldc3 divdc3 mulxc3 divxc3)
  set(bitcode "${CMAKE_CURRENT_BINARY_DIR}/complex/${name}.bc")
  add_custom_command(OUTPUT "${bitcode}"
    COMMAND "${BENNETT_CLANG}" ${complex_flags} -DL_${name}
            "${libgcc_source}/libgcc/libgcc2.c" -o "${bitcode}"
    DEPENDS "${libgcc_source}/libgcc/libgcc2.c" ${libgcc_headers}
    VERBATIM)
  list(APPEND complex_bitcode "${bitcode}")
endforeach()
add_custom_command(OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/complex-runtime.bc"
  COMMAND "${BENNETT_LLVM_LINK}" ${complex_bitcode} -o "${CMAKE_CURRENT_BINARY_DIR}/complex-runtime.bc"
  DEPENDS ${complex_bitcode} VERBATIM)
add_custom_target(complex-runtime ALL DEPENDS "${CMAKE_CURRENT_BINARY_DIR}/complex-runtime.bc")

# Exhaustive attribute policy and the public header's C++ translation unit.
add_executable(bennett-front-checks tests/checks.cpp)
target_include_directories(bennett-front-checks PRIVATE include src)
target_include_directories(bennett-front-checks SYSTEM PRIVATE ${LLVM_INCLUDE_DIRS})
target_compile_options(bennett-front-checks PRIVATE -Wall -Wextra -Wswitch-enum -Werror)
if(LLVM_LINK_LLVM_DYLIB)
  target_link_libraries(bennett-front-checks PRIVATE LLVM)
else()
  target_link_libraries(bennett-front-checks PRIVATE ${llvm_libs})
endif()
