# Rust JSG Integration — Design Document

**Status**: Active development  
**Last updated**: 2026-03-24  
**Owner**: @anonrig

---

## Overview

This document describes the current state of the Rust JSG (JavaScript Glue) integration in
workerd — the system that allows Rust code to expose types and methods to the JavaScript runtime
without writing C++ bindings. It covers the architecture, the type system, the GC integration,
the proc-macro layer, and the in-progress work tracked in open pull requests.

---

## Motivation

workerd's C++ JSG layer (`src/workerd/jsg/`) is the canonical way to expose native types to
JavaScript. It is powerful but requires significant boilerplate and deep familiarity with V8
internals. As more of workerd's implementation moves to Rust (Node.js compat APIs, DNS, crypto,
transpilation), a Rust-native binding layer reduces the friction of writing new APIs and
eliminates the need for a C++ shim for every Rust-backed feature.

The Rust JSG layer mirrors the C++ JSG design closely:

| C++ JSG concept | Rust JSG equivalent |
|---|---|
| `JSG_RESOURCE_TYPE` | `#[jsg_resource]` |
| `JSG_STRUCT` | `#[jsg_struct]` |
| `JSG_METHOD` | `#[jsg_method]` |
| `JSG_STATIC_CONSTANT` | `#[jsg_static_constant]` |
| `JSG_PROTOTYPE_PROPERTY` / `JSG_READONLY_PROTOTYPE_PROPERTY` | `#[jsg_prototype_property]` |
| `JSG_INSTANCE_PROPERTY` / `JSG_LAZY_INSTANCE_PROPERTY` | `#[jsg_instance_property]` / `#[jsg_instance_property(lazy)]` |
| `JSG_INSPECT_PROPERTY` | `#[jsg_inspect_property]` |
| `jsg::Ref<T>` / `jsg::JsRef<T>` | `jsg::Rc<T>` |
| `jsg::V8Ref<T>` / `jsg::Data` | `jsg::v8::Global<T>` |
| `kj::OneOf<…>` | `#[jsg_oneof]` enum |
| `jsg::Lock&` | `&mut jsg::Lock` |

---

## Architecture

### Crate Layout

```
src/rust/
├── jsg/           # Core runtime: Lock, Rc<T>, Realm, v8 bindings, wrappable bridge
│   ├── lib.rs     # Lock, Error, Nullable, NonCoercible, Number, Realm, Type, GarbageCollected
│   ├── v8.rs      # Local<T>, Global<T>, IsolatePtr, TypedArray types, CXX bridge
│   ├── resource.rs # Rc<T>, Weak<T>, Resource trait, Resources cache, wrap()
│   ├── wrappable.rs # ToJS, FromJS trait impls for all primitive/collection types
│   ├── modules.rs  # Module registration helpers
│   ├── feature_flags.rs # FeatureFlags (compat flags reader)
│   ├── ffi.c++    # C++ side of the CXX bridge
│   └── ffi.h      # C++ declarations
├── jsg-macros/    # Proc macros: #[jsg_resource], #[jsg_method], etc.
│   ├── lib.rs     # Entry points (thin dispatchers)
│   ├── resource.rs # Code generation for #[jsg_resource] (in PR #6384)
│   ├── trace.rs   # GC trace code generation (in PR #6384)
│   ├── traceable.rs # #[jsg_traceable] / #[jsg_trace] (in PR #6384)
│   └── utils.rs   # Shared helpers (in PR #6384)
└── jsg-test/      # Test harness: Harness, run_in_context()
    └── tests/     # Unit tests for all binding types
```

### Layer Diagram

```
JavaScript (V8)
      │
      │  FunctionCallbackInfo / Local<T>
      ▼
┌─────────────────────────────────────────────────────┐
│  C++ JSG layer  (src/workerd/jsg/)                  │
│  Wrappable, CppgcShim, FunctionTemplate, Realm      │
└──────────────────────┬──────────────────────────────┘
                       │  CXX bridge (ffi.c++ / ffi.h)
                       ▼
┌─────────────────────────────────────────────────────┐
│  Rust JSG runtime  (src/rust/jsg/)                  │
│  Lock, Rc<T>, Realm, v8::Local<T>, v8::Global<T>    │
└──────────────────────┬──────────────────────────────┘
                       │  proc macros
                       ▼
┌─────────────────────────────────────────────────────┐
│  Rust API implementations  (src/rust/api/, etc.)    │
│  #[jsg_resource] structs, #[jsg_method] impls       │
└─────────────────────────────────────────────────────┘
```

---

## Core Types

### `Lock`

`jsg::Lock` is the Rust equivalent of `jsg::Lock&` in C++. It is a proof token that the current
thread holds the V8 isolate lock. It is `!Send + !Sync` and cannot escape the thread that created
it. All V8 operations require a `&mut Lock`.

Key methods:
- `lock.isolate()` — returns the raw `IsolatePtr`
- `lock.realm()` — returns the per-isolate `Realm` (mutable)
- `lock.feature_flags()` — returns the capnp `CompatibilityFlags` reader
- `lock.throw_exception(&err)` — schedules a JS exception
- `lock.throw_internal_error(msg)` — logs internally, throws a generic JS error
- `lock.request_termination()` — terminates JS execution (mirrors `KJ_ASSERT` behavior)

### `Rc<R>` and `Weak<R>`

`jsg::Rc<R>` is the Rust equivalent of both `jsg::Ref<T>` (GC-integrated reference to a
resource) and `jsg::JsRef<T>` (persistent JS value reference) in C++ — `jsg::JsRef<T>` is not
needed as a separate type in Rust. It holds:
- An `std::rc::Rc<R>` for Rust-side shared ownership
- A `WrappableRc` (`KjRc<Wrappable>`) for GC integration

The `Wrappable` stores a fat pointer (`TraitObjectPtr`) to `dyn GarbageCollected` — the data
part is `Rc::into_raw(*const R)` and the vtable carries `R`'s `GarbageCollected` impl.

**GC lifecycle:**
1. `Rc::new(resource)` — creates the Rust `Rc`, leaks a clone as a fat pointer, creates a
   `Wrappable` on the KJ heap.
2. `rc.to_js(lock)` — wraps the resource in a V8 object backed by the cached `FunctionTemplate`.
   V8 caches the wrapper on the `Wrappable` via `CppgcShim` — multiple `to_js()` calls on clones
   of the same `Rc` return the **same** JS object.
3. `Rc::drop` — calls `wrappable_remove_strong_ref(is_strong)` → C++ `maybeDeferDestruction()`.
4. When all Rust `Rc`s are dropped and the JS wrapper becomes unreachable, V8 GC collects the
   `CppgcShim`, which calls `wrappable_invoke_drop` → reconstructs the `Rc` via `Rc::from_raw`
   and drops it.

`jsg::Weak<R>` holds an `std::rc::Weak<R>` and a non-owning raw pointer to the `Wrappable`.
`upgrade()` checks liveness via `Weak::strong_count()` before dereferencing the pointer.

### `Realm`

`Realm` is per-isolate state. It holds:
- `resources: Resources` — a `HashMap<TypeId, Global<FunctionTemplate>>` caching one
  `FunctionTemplate` per resource type
- `feature_flags: FeatureFlags` — parsed capnp `CompatibilityFlags` bytes, initialized once at
  worker startup via `realm_create()`

### `v8::Local<'a, T>` and `v8::Global<T>`

`Local<'a, T>` is a stack-allocated V8 handle. The lifetime `'a` is tied to the `HandleScope`.
`Global<T>` is a persistent handle that outlives `HandleScope`s.

`Global<T>` fields on `#[jsg_resource]` structs participate in GC tracing via
`wrappable_visit_global`, implementing the same strong↔traced dual-mode as C++ `jsg::V8Ref<T>`.
This enables cycle collection for patterns like a resource holding a JS callback that closes over
its own wrapper.

---

## Type Conversion System

### `ToJS` and `FromJS`

All type conversions go through two traits in `wrappable.rs`:

```rust
pub trait ToJS: Sized {
    fn to_js<'a, 'b>(self, lock: &'a mut Lock) -> v8::Local<'b, v8::Value> where 'b: 'a;
}

pub trait FromJS: Sized {
    type ResultType;
    fn from_js(lock: &mut Lock, value: v8::Local<v8::Value>) -> Result<Self::ResultType, Error>;
}
```

### Supported Type Mappings

| Rust Type | JavaScript Type | Notes |
|---|---|---|
| `String` / `&str` | `string` | `&str` params convert via owned `String` |
| `bool` | `boolean` | |
| `jsg::Number` | `number` | Wrapper around `f64`; distinct from `f64` (used for `Float64Array`) |
| `u8`..`i32` | `number` | Truncating conversion; use `NonCoercible<T>` for strict |
| `()` | `undefined` | |
| `Option<T>` | `T \| undefined` | Rejects `null` |
| `Nullable<T>` | `T \| null \| undefined` | Three-way enum |
| `NonCoercible<T>` | `T` (strict) | Rejects values requiring coercion |
| `Vec<T: ToJS>` | `Array<T>` | Generic JS array |
| `Vec<u8>` | `Uint8Array` | Zero-copy on `to_js`; copies on `from_js` |
| `Vec<u16..u64, i8..i64, f32..f64>` | Typed arrays | Same pattern |
| `&[T]` | Typed array | Parameter-only; delegates to `Vec<T>` |
| `Local<T: TypedArray>` | Typed array | Zero-copy slice access via `as_slice()` / `as_mut_slice()` (PR #6386) |
| `Local<ArrayBuffer>` | `ArrayBuffer` | `new`, `new_uninit`, `as_slice`, `as_mut_slice`, `to_vec`, `backing_store()` (PR #6398) |
| `Local<ArrayBufferView>` | `ArrayBuffer \| TypedArray \| DataView` | Supertype of all typed arrays and `DataView`; `as_slice`, `as_mut_slice`, `buffer()` (PR #6398) |
| `Local<SharedArrayBuffer>` | `SharedArrayBuffer` | Same API surface as `ArrayBuffer`; `backing_store().is_shared() == true` (PR #6398) |
| `BackingStore` | — | Owned handle to the raw backing memory; outlives the `Local`; `as_slice`, `as_mut_slice`, `is_shared`, `is_resizable_by_user_javascript` (PR #6398) |
| `jsg::Rc<R>` | Resource object | Reference type; GC-integrated |
| `T: Struct` | `object` | Value type; deep-copied |
| `#[jsg_oneof] enum` | Union type | Tries variants in order |

### `jsg::Error`

`jsg::Error` carries an `ExceptionType` (mirrors C++ `jsg::DOMException` subtypes) and a message
string. Constructor helpers: `Error::new_type_error(msg)`, `Error::new_range_error(msg)`, etc.
`Result<T, jsg::Error>` return types automatically throw a JS exception on `Err`.

---

## Proc Macro Layer (`jsg-macros`)

### `#[jsg_resource]`

Applied to both struct definitions and impl blocks.

**On a struct** — generates:
- `jsg::Type` (class name, `is_exact`)
- `jsg::ToJS` (wraps via `resource::wrap()`)
- `jsg::FromJS` (unwraps via `WrappableRc::resolve_resource::<R>()`)
- `jsg::GarbageCollected` (auto-synthesised `trace()` body — see GC section)

**On an impl block** — generates:
- `jsg::Resource::members()` — collects all `#[jsg_method]`, `#[jsg_constructor]`, and
  `#[jsg_static_constant]` items into a `Vec<Member>` used to build the `FunctionTemplate`

Options:
- `#[jsg_resource(name = "JSName")]` — override the JavaScript class name
- `#[jsg_resource(custom_trace)]` — suppress auto-generated `GarbageCollected`; user provides
  their own impl (added in PR #6384)

### `#[jsg_method]`

Generates a `extern "C" fn` V8 callback for a method. The macro:
1. Extracts arguments from `FunctionCallbackInfo` via `FromJS`
2. Calls the Rust method
3. Converts the return value via `ToJS` and sets it on the callback info
4. Wraps the whole body in `jsg::catch_panic()` so Rust panics become internal JS errors

- Methods with `&self` / `&mut self` → instance methods (on prototype)
- Methods without a receiver → static methods (on constructor)
- `snake_case` → `camelCase` automatically; override with `#[jsg_method(name = "jsName")]`
- First parameter may be `&mut Lock` — not exposed as a JS argument

### `#[jsg_constructor]`

Marks a static method (no receiver, returns `Self`) as the JS constructor. Without it,
`new MyClass()` throws `Illegal constructor`. Only one per impl block.

### `#[jsg_static_constant]`

Exposes a `const` item as a read-only property on both constructor and prototype.
Name is used as-is (no camelCase). Only numeric types supported.

### `#[jsg_struct]`

Generates `jsg::Struct`, `jsg::Type`, `jsg::ToJS`, and `jsg::FromJS` for a plain data struct.
Only `pub` fields are projected into the JavaScript object. Value semantics — deep-copied on
every conversion.

### `#[jsg_oneof]`

Generates `jsg::Type` and `jsg::FromJS` for a union enum. Variants are tried in declaration
order using exact-type matching. If none matches, a `TypeError` is thrown listing all expected
types.

### `#[jsg_prototype_property]`, `#[jsg_instance_property]`, `#[jsg_inspect_property]` (PR #6401)

Three macros for exposing Rust methods as JavaScript property accessors. All three reuse
`#[jsg_method]`'s callback generation internally; registration happens via `Member::Property`
rather than `Member::Method`.

**Setter detection** — a method whose Rust name starts with `set_` is registered as the setter.
All others are getters. Omitting a setter makes the property read-only.

**Naming** — `snake_case` → `camelCase` after stripping a leading `get_`/`set_` prefix.
Override with `#[jsg_*_property(name = "jsName")]`.

**`spec_compliant_property_attributes` compat flag** — when enabled, getter `.length = 0`,
setter `.length = 1`, getter `.name = "get <name>"`, setter `.name = "set <name>"` per Web IDL §3.7.6.

| Macro | C++ equivalent | Enumerability | Own property? | Notes |
|---|---|---|---|---|
| `#[jsg_prototype_property]` | `JSG_PROTOTYPE_PROPERTY` / `JSG_READONLY_PROTOTYPE_PROPERTY` | Not in `Object.keys()`; visible via `"prop" in obj` | No | Prefer this in almost all cases |
| `#[jsg_instance_property]` | `JSG_INSTANCE_PROPERTY` / `JSG_LAZY_INSTANCE_PROPERTY` | In `Object.keys()`; `hasOwnProperty()` returns `true` | Yes | Prevents minor-GC; inhibits V8 optimisations — use sparingly |
| `#[jsg_inspect_property]` | `JSG_INSPECT_PROPERTY` | Invisible to all string-key access | No | Registered under a unique symbol; surfaced only by `node:util` `inspect()` / `console.log()`; always read-only |

`#[jsg_instance_property(lazy)]` — getter called once on first access, result cached. Always
read-only; pairing `lazy` with a `set_*` method is a compile error.

### `#[jsg_traceable]` and `#[jsg_trace]` (PR #6384)

`#[jsg_traceable]` generates `GarbageCollected` for plain structs and enums that are not
themselves resources but contain GC-visible fields. Useful for the `kj::OneOf` state-machine
pattern where a resource's state is an enum.

`#[jsg_trace]` is a field attribute on `#[jsg_resource]` or `#[jsg_traceable]` types that
delegates GC tracing to the field type. The field type must implement `GarbageCollected`.

---

## Garbage Collection

### Auto-Traced Field Types

`#[jsg_resource]` on a struct synthesises a `GarbageCollected::trace` body. The following field
shapes are recognised and traced automatically:

| Field type | Traced? | Notes |
|---|---|---|
| `jsg::Rc<T>` | Yes — strong edge | Keeps target alive |
| `Option<jsg::Rc<T>>` | Yes — when `Some` | |
| `jsg::Nullable<jsg::Rc<T>>` | Yes — when `Some` | |
| `Vec<jsg::Rc<T>>` | Yes — each element | Added in PR #6384 |
| `HashMap<K, jsg::Rc<T>>` | Yes — each value | Added in PR #6384 |
| `BTreeMap<K, jsg::Rc<T>>` | Yes — each value | Added in PR #6384 |
| `HashSet<jsg::Rc<T>>` | Yes — each element | Added in PR #6384 |
| `BTreeSet<jsg::Rc<T>>` | Yes — each element | Added in PR #6384 |
| `jsg::v8::Global<T>` | Yes — dual strong/traced | Enables cycle collection |
| `Option<jsg::v8::Global<T>>` | Yes — when `Some` | |
| `Cell<F>` (any of the above) | Yes | Required for fields set after construction |
| `jsg::Weak<T>` | No | Does not keep target alive |
| Anything else | No | Plain data, ignored |

### Cycle Collection

`jsg::v8::Global<T>` is the Rust equivalent of both `jsg::V8Ref<T>` and `jsg::Data` in C++.
It supports GC tracing and uses the same strong↔traced dual-mode as C++ `jsg::V8Ref<T>`:
- While the parent resource has strong Rust `Rc` refs, the handle stays strong.
- Once all `Rc`s are dropped, `visit_global` downgrades the handle to a `v8::TracedReference`
  that cppgc can follow — allowing back-reference cycles to be collected.

### Circular References

Circular references through `jsg::Rc<T>` are **not** collected, matching C++ `jsg::Rc<T>`
behavior. Use `jsg::Weak<T>` to break cycles.

---

## Panic Behavior

All `extern "C"` V8 callbacks generated by `jsg-macros` are wrapped in `jsg::catch_panic()`.
When a Rust panic fires inside a callback:

1. The panic payload is captured via `std::panic::catch_unwind`.
2. The internal message is logged via `KJ_LOG(ERROR)`, reaching Sentry with a unique reference
   ID. The raw panic string is never exposed to JavaScript.
3. A generic `"Error: internal error; reference = <id>"` JS exception is thrown via
   `lock.throw_internal_error()`.
4. `lock.request_termination()` is called — this calls `IsolateBase::requestTermination()`,
   which sets the `terminationRequested` flag (checked by C++ iterator callbacks) and calls
   `v8::Isolate::TerminateExecution()`. V8 raises an uncatchable termination exception that
   unwinds all JS call frames back to the top-level C++ entry point.

This mirrors what C++ `KJ_ASSERT` / `KJ_FAIL_ASSERT` effectively do when they fire inside an
isolate context. Step 4 closes the window where a panicked resource with partially-mutated
`jsg::Rc` fields could be observed by a subsequent GC trace before isolate teardown.

`Lock` exposes two methods for this:

```rust
lock.request_termination();          // calls IsolateBase::requestTermination()
lock.is_termination_requested();     // mirrors IsolateBase::isTerminationRequested()
```

**PR [#6392](https://github.com/cloudflare/workerd/pull/6392)** (merged) added
`lock.request_termination()` and `lock.is_termination_requested()` to the `Lock` API, wired
`request_termination()` into `catch_panic()`, and updated the test to verify that the isolate
is terminated after a panic — not just that a JS error was thrown. The PR description notes that
when iterator support is added to Rust JSG, iterators must also check `terminationRequested`
(matching the C++ iterator pattern) to stop execution after a panic mid-iteration.

---

## Feature Flags (Compatibility Flags)

`Lock::feature_flags()` returns a capnp `compatibility_flags::Reader` for the current worker.
Flags are parsed once from canonical capnp bytes at `realm_create()` time and cached in the
`Realm`. No copies or re-parsing on access.

```rust
if lock.feature_flags().get_node_js_compat() {
    // Node.js compatibility behavior
}
```

---

## Module Registration

Rust modules are registered via `register_add_builtin_module()` in the CXX bridge. The `api/`
crate exposes `register_nodejs_modules()` which is called from C++ during isolate initialization.

---

## In-Progress Pull Requests

| PR | Title | Notes |
|---|---|---|
| [#6384](https://github.com/cloudflare/workerd/pull/6384) | Traceable collections + `#[jsg_traceable]` / `#[jsg_trace]` + macro refactor | Adds collection tracing (`Vec`, `HashMap`, etc.) and custom-trace escape hatch; under review by @jasnell |
| [#6386](https://github.com/cloudflare/workerd/pull/6386) | TypedArray zero-copy methods (`as_slice`, `as_mut_slice`, `byte_offset`, `data`) | `as_mut_slice` has an aliasing hazard — two `Local` handles can alias the same buffer |
| [#6398](https://github.com/cloudflare/workerd/pull/6398) | Add `ArrayBuffer`, `ArrayBufferView`, `SharedArrayBuffer`, `BackingStore` | Built on top of #6386; see below |
| [#6401](https://github.com/cloudflare/workerd/pull/6401) | Add instance/prototype/inspect properties to Rust JSG | `#[jsg_prototype_property]`, `#[jsg_instance_property]` (with `lazy`), `#[jsg_inspect_property]`; 1012-line test suite |

---

## Known Gaps and Future Work

The gaps below were identified during review of PR #6272 (merged) and the open follow-up PRs.
@jasnell flagged the GC tracing gaps as **blockers for migrating a large number of C++ APIs to
Rust** — they must be resolved before complex resources like streams, event targets, or crypto
keys can be ported.

### GC Tracing Gaps (Blockers for API Migration)

**Nested struct delegation** ([jasnell comment](https://github.com/cloudflare/workerd/pull/6272#issuecomment-4100679554))

The C++ codebase commonly holds GC-visible state in a private nested struct and delegates
`visitForGc` to it. The Rust macro has no equivalent — a field of a user-defined struct type is
classified as `TraceableType::None` and silently skipped, even if that struct contains `Rc<T>`
or `Global<T>` fields. This pattern appears in ~38 places in `src/workerd/api/` alone
(`ReadableImpl::Algorithms`, `WritableImpl::WriteRequest`, `CryptoKey::Impl`, `EventTarget`
handlers, etc.).

PR #6384 addresses this with `#[jsg_trace]` (field-level delegation) and
`#[jsg_resource(custom_trace)]` (full escape hatch). The `#[jsg_traceable]` macro generates
`GarbageCollected` for the nested struct itself.

**Collection tracing** ([jasnell comment](https://github.com/cloudflare/workerd/pull/6272#issuecomment-4100794974))

`Vec<jsg::Rc<T>>`, `Vec<jsg::v8::Global<T>>`, `HashMap<K, jsg::Rc<V>>`, and similar collection
types were classified as `TraceableType::None` and silently skipped. C++ provides
`GcVisitor::visitAll(collection)` for this. Affects `Performance`, `TraceItem`, `QueueEvent`,
`DiagnosticsChannelModule`, `EventTarget`, `HTMLRewriter`, and others.

PR #6384 addresses this by extending the auto-generated trace body to handle `Vec`, `HashMap`,
`BTreeMap`, `HashSet`, and `BTreeSet` of traceable types.

**Enum / state-machine tracing** ([jasnell comment](https://github.com/cloudflare/workerd/pull/6272#issuecomment-4100822217))

The C++ codebase uses `kj::OneOf<T...>` extensively for state machines where different variants
hold different GC-visible fields (e.g., stream controller states: `Closed`, `Errored`,
`Readable`). The Rust macro had no support for tracing enum variants.

PR #6384 addresses this with `#[jsg_traceable]` on enums, which generates one `match` arm per
variant.

**Type alias / newtype invisibility** ([jasnell comment](https://github.com/cloudflare/workerd/pull/6272#issuecomment-4100815513))

Proc macros operate on the syntactic AST, not the resolved type system. A field declared as
`callback: MyCallback` where `type MyCallback = jsg::v8::Global<jsg::v8::Function>` will be
seen by the macro as a single path segment `MyCallback`, which does not match `jsg::v8::Global`.
The field is silently ignored — a potential GC safety hole.

**Mitigation**: GC-visible fields must use canonical type paths (`jsg::Rc<T>`,
`jsg::v8::Global<T>`, etc.) directly. Type aliases and newtype wrappers around GC-visible types
must use `#[jsg_trace]` or `#[jsg_resource(custom_trace)]` to ensure tracing.

**Trait-based tracing design** ([guybedford comment](https://github.com/cloudflare/workerd/pull/6272#issuecomment-4100179829))

@guybedford suggested replacing the macro's field-type pattern matching with a `Trace` trait
(analogous to Rust's `Drop` or `serde::Serialize`). The macro would emit a uniform
`Trace::trace(&self.field, visitor)` call for every field, and missing `Trace` impls would be
compile errors rather than silent skips. This was agreed as a desirable follow-up but not
required for the current PRs.

### Missing C++ JSG Types

The following C++ JSG types have no Rust equivalent yet and cannot be stored in or traced from
Rust resources ([jasnell comment](https://github.com/cloudflare/workerd/pull/6272#issuecomment-4100806386)):

| C++ type | Notes |
|---|---|
| `jsg::Function<Ret(Args...)>` | GC-traced callable; used heavily in streams, event handlers. `v8::Function` (`Local<Function>`) exists for calling raw JS functions, but there is no `jsg::Function` equivalent that is GC-traceable and type-safe as a stored field |
| `jsg::Promise<T>` / `jsg::Promise<T>::Resolver` | No Rust-native promise type; `Lock::await_io()` is the intended path but is unimplemented |
| `jsg::Name` | Supertype of `String` and `Symbol`; `v8::Name` and `v8::Symbol` were added (PR #6388, merged) but there is no JSG-level `jsg::Name` type yet |
| `jsg::JsRef<T>` | Not needed — `jsg::Rc<R>` covers this role (see below) |
| `jsg::MemoizedIdentity<T>` | Cached JS wrapper identity |
| `jsg::HashableV8Ref<T>` | Hashable persistent JS reference |
| `jsg::BufferSource` | Accepts `ArrayBuffer` or `ArrayBufferView`; PR #6398 adds both handle types, but no `jsg::BufferSource` union type yet — callers must accept `Local<ArrayBuffer>` or `Local<ArrayBufferView>` separately |
| `jsg::PromiseResolverPair<T>` | Promise + resolver pair |

### Other Gaps

| Feature | Notes |
|---|---|
| `#[jsg_property]` (getter/setter) | Added in PR #6401: `#[jsg_prototype_property]`, `#[jsg_instance_property]`, `#[jsg_inspect_property]` |
| `Lock::await_io()` | Stub that panics; async I/O from Rust resources not yet supported |
| `jsg::MemoryTracker` | `Type::memory_info()` is a TODO comment in `lib.rs` |
| `ObjectTemplate` caching for `#[jsg_struct]` | Recreates object shape on every `to_js()` call; C++ JSG uses a cached `ObjectTemplate` |
| Prototype inheritance (`#[jsg_resource(extends = "Base")]`) | No equivalent of C++ `JSG_RESOURCE_TYPE` `extends` |
| `BigInt` scalar type | `BigInt64Array` / `BigUint64Array` exist but no scalar `BigInt` |
| `ArrayBuffer` direct access | Added in PR #6398: `Local<ArrayBuffer>`, `Local<ArrayBufferView>`, `Local<SharedArrayBuffer>`, `BackingStore` |
| Iterator protocol | No `Symbol.iterator` / `for...of` support |
| `Float16Array` | No `Local<Float16Array>` type, no `Vec<f16>` `ToJS`/`FromJS`, no `&[f16]` parameter support |

### Design Considerations

**Mutable typed array parameters should use `&mut [T]`**: The current binding layer supports
`&[u8]` (and all other typed array slice types) as read-only method parameters, which is
idiomatic Rust. The natural extension for in-place mutation is `&mut [u8]` — a method that
accepts a `Uint8Array` and modifies it in place. This is not yet supported; the workaround is
`Local<Uint8Array>` with `as_mut_slice()` (added in PR #6386), but that syntax is non-idiomatic
and exposes V8 internals to API authors. Adding `FromJS` for `&mut [T]` would let methods be
written as `fn fill(&self, buf: &mut [u8])` instead of
`fn fill(&self, mut buf: Local<Uint8Array>)`.

**`as_mut_slice` aliasing**: The zero-copy mutable slice API in PR #6386 has an inherent aliasing
hazard — two `Local<T>` handles can point to the same `ArrayBuffer`. The current safety
documentation understates this. Consider whether `as_mut_slice` should require a stronger
invariant (e.g., consuming the `Local`) or whether the documentation should be more explicit.

**`jsg_struct` performance**: Every `to_js()` call on a `#[jsg_struct]` creates a new V8 object
from scratch. C++ JSG uses a cached `ObjectTemplate` to pre-define the object shape. This is a
known TODO and matters for hot-path APIs that return structs frequently.

**`#[jsg_oneof]` ordering**: Variants are tried in declaration order. For overlapping types
(e.g., `Number` before `String`), the order matters. This matches C++ `kj::OneOf` behavior but
is not enforced by the type system.

**Circular `Rc` references**: Like C++ `jsg::Rc<T>`, circular references through `jsg::Rc<T>`
are not collected. The `jsg::v8::Global<T>` cycle-collection mechanism only handles back-edges
from JS values to their owning resource. Pure Rust `Rc` cycles leak until worker teardown.

**Design principle** (@kentonv): The Rust JSG layer should transliterate C++ JSG patterns, not
invent new ones. Where the language differs (e.g., proc macros vs. C++ macros), the design may
diverge, but the GC tracing semantics and resource lifecycle must match C++ exactly.

---

## Testing

Tests live in `src/rust/jsg-test/tests/`. Each test file covers a specific type or feature:

| File | Coverage |
|---|---|
| `arrays.rs` | TypedArray `ToJS`/`FromJS`, zero-copy slice methods (PR #6386) |
| `eval.rs` | `ctx.eval()` / `ctx.eval_raw()` harness helpers |
| `function.rs` | `Local<Function>::call()` |
| `gc.rs` | Basic GC lifecycle for `Rc<T>` and `Weak<T>` |
| `jsg_oneof.rs` | `#[jsg_oneof]` union type dispatch |
| `jsg_struct.rs` | `#[jsg_struct]` value type round-trips |
| `local_cast.rs` | `Local<T>` upcast/downcast via `try_as` and `From` |
| `name.rs` | `Local<Name>` identity hash, upcasts (PR #6388) |
| `non_coercible.rs` | `NonCoercible<T>` strict type checking |
| `resource_callback.rs` | `#[jsg_method]` callbacks, error propagation |
| `resource_conversion.rs` | `Rc<T>` `to_js` / `from_js` round-trips |
| `string.rs` | `Local<String>` construction, encoding, comparison |
| `symbol.rs` | `Local<Symbol>` creation, description, global registry (PR #6388) |
| `unwrap.rs` | Primitive `FromJS` conversions |
| `collections_gc.rs` | Collection tracing: `Vec`, `HashMap`, etc. (PR #6384) |
| `traceable_gc.rs` | `#[jsg_traceable]` / `#[jsg_trace]` (PR #6384) |

Tests use `jsg_test::Harness::run_in_context(|lock, ctx| { … })` which sets up a full V8
isolate with a `HandleScope` and a `Realm`.

---

## Conventions Reference

- `&Lock` / `&mut Lock` must always be the **first** parameter (after `&self`/`&mut self`)
- `#[jsg_method]` converts `snake_case` → `camelCase` automatically
- `#[jsg_static_constant]` names are used as-is (no camelCase)
- FFI functions receiving raw pointers must be `unsafe fn`
- `jsg::Error` (not `std::error::Error`) for all JSG-facing error types
- `thiserror` for library-internal errors that are then converted to `jsg::Error`
- Clippy: pedantic + nursery; `allow-unwrap-in-tests`; `fn_params_excessive_bools` enabled (merged via PR #6387)
