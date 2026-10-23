# liblillith_llm.so: on-device llama.cpp behind a tiny JNI shim.
#
# Fully local: the model is a GGUF file the user side-loads and picks in
# first-run setup. Nothing here opens a socket or downloads anything.
# llama.cpp is built CPU-only (no OpenMP, no curl/OpenSSL, no tools/server).
cmake_minimum_required(VERSION 3.22)
project(lillith_llm C CXX)

if(NOT DEFINED LLAMA_CPP_DIR)
  message(FATAL_ERROR "pass -DLLAMA_CPP_DIR=/path/to/llama.cpp (see android/build-apk.sh)")
endif()

set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_COMMON OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_TOOLS OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_SERVER OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_APP OFF CACHE BOOL "" FORCE)
set(LLAMA_CONNECT OFF CACHE BOOL "" FORCE)
set(LLAMA_OPENSSL OFF CACHE BOOL "" FORCE)
set(LLAMA_CURL OFF CACHE BOOL "" FORCE)
set(GGML_OPENMP OFF CACHE BOOL "" FORCE)
set(GGML_NATIVE OFF CACHE BOOL "" FORCE)
set(GGML_LLAMAFILE OFF CACHE BOOL "" FORCE)

add_subdirectory(${LLAMA_CPP_DIR} llama.cpp EXCLUDE_FROM_ALL)

add_library(lillith_llm SHARED lillith_llm.c)
target_link_libraries(lillith_llm PRIVATE llama ggml log android)
target_link_options(lillith_llm PRIVATE -Wl,--gc-sections -Wl,-z,max-page-size=16384)
