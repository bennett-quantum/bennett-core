# bennett-front: LLVM bitcode in, Bennett device program text out. The only part of the project that links
# LLVM. Configure with the LLVM 18 CMake package, for example
#   cmake -S front -B target/front -DLLVM_DIR="$(llvm-config-18 --cmakedir)"
cmake_minimum_required(VERSION 3.20)
project(bennett-front LANGUAGES CXX)

find_package(LLVM 18.1 REQUIRED CONFIG)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release)
endif()

add_executable(bennett-front
  src/main.cpp
  src/Diagnostic.cpp
  src/Markers.cpp
  src/Normalise.cpp
  src/Taint.cpp
  src/Outline.cpp
  src/Emit.cpp
  src/Control.cpp
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
