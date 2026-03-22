// Copyright (c) 2025 Cloudflare, Inc.
// Licensed under the Apache 2.0 license found in the LICENSE file or at:
//     https://opensource.org/licenses/Apache-2.0

#include "sqlite-metering.h"

#include <sqlite3.h>

#include <kj/debug.h>

#include <cstdlib>

#if defined(__linux__) || defined(__GLIBC__)
#include <malloc.h>
#define SQLITE_METERING_MALLOC_USABLE_SIZE(p) malloc_usable_size(p)
#elif defined(__APPLE__)
#include <malloc/malloc.h>
#define SQLITE_METERING_MALLOC_USABLE_SIZE(p) malloc_size(p)
#elif defined(_WIN32)
#include <malloc.h>
#define SQLITE_METERING_MALLOC_USABLE_SIZE(p) _msize(p)
#else
// Fallback: no accounting. This path is unreachable in production (Linux).
#define SQLITE_METERING_MALLOC_USABLE_SIZE(p) 0
#endif

namespace workerd {

thread_local SqliteMemoryScope* SqliteMemoryScope::threadLocalScope = nullptr;

namespace {

inline size_t usableSize(void* ptr) {
  if (ptr == nullptr) return 0;
#if defined(SQLITE_METERING_MALLOC_USABLE_SIZE)
  auto sz = SQLITE_METERING_MALLOC_USABLE_SIZE(ptr);
  if (sz > 0) return sz;
#endif
  // Fallback: no accounting. This path is unreachable in production (Linux).
  return 0;
}

}  // namespace

void* sqliteMemMalloc(int size) {
  if (size <= 0) return nullptr;
  void* ptr = std::malloc(static_cast<size_t>(size));
  if (ptr == nullptr) return nullptr;
  SqliteMemoryScope* scope = SqliteMemoryScope::threadLocalScope;
  if (scope != nullptr) {
    size_t actual = usableSize(ptr);
    if (actual > 0) {
      if (scope->counter + actual >= scope->hardLimitBytes) {
        std::free(ptr);
        return nullptr;
      }
      scope->counter += actual;
    }
  }
  return ptr;
}

void sqliteMemFree(void* ptr) {
  if (ptr == nullptr) return;
  SqliteMemoryScope* scope = SqliteMemoryScope::threadLocalScope;
  if (scope != nullptr) {
    size_t actual = usableSize(ptr);
    if (actual > 0) {
      scope->counter -= actual;
    }
  }
  std::free(ptr);
}

void* sqliteMemRealloc(void* ptr, int newSize) {
  // SQLite assumes that sqliteMemRealloc(ptr, newSize) where newSize <= 0 is equivalent to
  // sqliteMemFree(ptr).
  if (newSize <= 0) {
    sqliteMemFree(ptr);
    return nullptr;
  }

  // SQLite assumes that sqliteMemRealloc(nullptr, newSize) is equivalent to
  // sqliteMemMalloc(newSize).
  if (ptr == nullptr) {
    return sqliteMemMalloc(newSize);
  }

  SqliteMemoryScope* scope = SqliteMemoryScope::threadLocalScope;
  size_t oldActual = (scope != nullptr) ? usableSize(ptr) : 0;

  // sqliteMemRealloc must leave the original buffer intact per SQLite's contract when returning
  // a nullptr. Because std::realloc consumes the original pointer, we must check whether the
  // requested size would exceed the limit before calling std::realloc.
  if (scope != nullptr && oldActual > 0 && static_cast<size_t>(newSize) > oldActual) {
    size_t growth = static_cast<size_t>(newSize) - oldActual;
    if (scope->counter + growth >= scope->hardLimitBytes) {
      return nullptr;
    }
  }

  void* newPtr = std::realloc(ptr, static_cast<size_t>(newSize));
  if (newPtr == nullptr) return nullptr;

  if (scope != nullptr && oldActual > 0) {
    size_t newActual = usableSize(newPtr);
    // Note that `scope->counter + (newActual - oldActual) >= scope->hardLimitBytes` could still be
    // true. If we attempt to restore a memory block of the original size then the original data
    // would be lost.
    scope->counter = scope->counter - oldActual + newActual;
  }
  return newPtr;
}

namespace {

int sqliteMemSize(void* ptr) {
  return static_cast<int>(usableSize(ptr));
}

int sqliteMemRoundup(int n) {
  // Round up to the next multiple of 8 as a conservative estimate.
  return (n + 7) & ~7;
}

int sqliteMemInit(void* /*pAppData*/) {
  return SQLITE_OK;
}

void sqliteMemShutdown(void* /*pAppData*/) {}

static const sqlite3_mem_methods kSqliteMemMethods = {
  sqliteMemMalloc,
  sqliteMemFree,
  sqliteMemRealloc,
  sqliteMemSize,
  sqliteMemRoundup,
  sqliteMemInit,
  sqliteMemShutdown,
  /*pAppData=*/nullptr,
};

}  // namespace

SqliteMemoryScope::SqliteMemoryScope(size_t& counter, size_t hardLimitBytes)
    : counter(counter),
      hardLimitBytes(hardLimitBytes) {
  KJ_ASSERT(
      threadLocalScope == nullptr, "Another SqliteMemoryScope is already active on this thread.");
  threadLocalScope = this;
}

SqliteMemoryScope::~SqliteMemoryScope() noexcept(false) {
  KJ_ASSERT(threadLocalScope == this);
  threadLocalScope = nullptr;
}

void installSqliteCustomAllocator() {
  // sqlite3_config() must be called before sqlite3_initialize(), which is itself invoked
  // implicitly by the first sqlite3_vfs_register() or sqlite3_open_v2() call. We rely on callers
  // (Vfs constructors, ensureMonkeyPatchedSqlite) to invoke this before any of those.
  static bool installed KJ_UNUSED = []() {
    int rc = sqlite3_config(SQLITE_CONFIG_MALLOC, &kSqliteMemMethods);
    KJ_ASSERT(rc == SQLITE_OK, "sqlite3_config(SQLITE_CONFIG_MALLOC) failed", rc);

    // Disable the global sqlite3_mem_used counter. It carries per-call mutex overhead and is
    // redundant now that per-actor accounting is done via the per-actor counter.
    rc = sqlite3_config(SQLITE_CONFIG_MEMSTATUS, 0);
    KJ_ASSERT(rc == SQLITE_OK, "sqlite3_config(SQLITE_CONFIG_MEMSTATUS) failed", rc);

    return true;
  }();
}

}  // namespace workerd
