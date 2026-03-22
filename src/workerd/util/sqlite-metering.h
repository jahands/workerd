// Copyright (c) 2025 Cloudflare, Inc.
// Licensed under the Apache 2.0 license found in the LICENSE file or at:
//     https://opensource.org/licenses/Apache-2.0

#pragma once

#include <kj/common.h>

#include <cstddef>

namespace workerd {

// This module implements per-actor SQLite memory metering.
//
// SQLite uses a single process-wide memory allocator, but we want to account allocations against
// the specific actor (Durable Object) on whose behalf they are made, and enforce a per-actor hard
// limit for memory allocations. We do this by:
//
// 1. Installing a custom sqlite3_mem_methods that wrap the system allocator.
//
// 2. Installing a thread-local SqliteMemoryScope object, via LimitEnforcerImpl::enterJs, for each
//    JS turn of an actor. The scope points at a size_t byte counter owned by ActorSqlite to meter
//    memory allocations.
//
// 3. When a SqliteMemoryScope is active on the current thread, we count each memory allocation
//    against the byte counter and enforce the per-actor hard limit by returning nullptr signalling
//    SQLite to throw a SQLITE_NOMEM exception.
class SqliteMemoryScope {
 public:
  // counter: the per-actor byte counter owned by SqliteDatabase for its lifetime.
  // hardLimitBytes: the per-actor cap from WorkerLimits::sqliteMaxMemoryMb.
  explicit SqliteMemoryScope(size_t& counter, size_t hardLimitBytes);
  ~SqliteMemoryScope() noexcept(false);
  KJ_DISALLOW_COPY_AND_MOVE(SqliteMemoryScope);

  size_t& counter;
  const size_t hardLimitBytes;

 private:
  // Thread-local pointer to the active scope. Set to this on construction, cleared on destruction.
  static thread_local SqliteMemoryScope* threadLocalScope;

  friend void* sqliteMemMalloc(int);
  friend void sqliteMemFree(void*);
  friend void* sqliteMemRealloc(void*, int);
};

// The custom sqlite3_mem_methods functions. Declared here so tests can call them directly to
// verify accounting behaviour without going through the SQLite query engine.
void* sqliteMemMalloc(int size);
void sqliteMemFree(void* ptr);
void* sqliteMemRealloc(void* ptr, int newSize);

// Install the custom sqlite3_mem_methods.
//
// This must be called before the first sqlite3_initialize(), sqlite3_open_v2(), or
// sqlite3_vfs_register() call in the process. Idempotent (uses a static once-flag).
void installSqliteCustomAllocator();

}  // namespace workerd
