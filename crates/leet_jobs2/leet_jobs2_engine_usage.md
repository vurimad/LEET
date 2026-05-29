# Using leet_jobs2 — Engine Code Guide

This file is for code that USES the job system, not the job system itself.
Examples here belong in your engine crates (render graph, physics, etc).
None of this belongs inside leet_jobs2.

---

## The boundary

```
leet_jobs2 (job system crate)        your engine crates
─────────────────────────────        ──────────────────
Counter                        ←use─  RenderCommandHandler
JobDecl                               RenderFrame
Builder                               PhysicsWorld
RunContext                            AudioSystem
CompletionDeferral                    etc.
```

The job system knows nothing about your engine types.
Your engine code knows about the job system types.

---

## Rust Concepts You Need

### `'static` as a bound

When you pass a closure to `builder.dispatch_job(...)`, Rust requires it to be `'static`.
This does NOT mean the closure lives forever.
It means: the closure owns everything inside it — nothing is borrowed from the stack.

```rust
// BAD — borrows local variable, not 'static
let local = 42;
builder.dispatch_job("name", |ctx| {
    println!("{}", local); // borrows local — COMPILE ERROR
});

// GOOD — moves the value in, closure owns it
let local = 42;
builder.dispatch_job("name", move |ctx| {
    println!("{}", local); // copied into closure — fine
});

// GOOD — Arc is 'static and Send
let data = Arc::new(42);
let data_clone = Arc::clone(&data);
builder.dispatch_job("name", move |ctx| {
    println!("{}", data_clone); // owned by closure — fine
});
```

### `Send`

Closures passed to `dispatch_job` must be `Send` — safe to move to another thread.
`Arc<T>` is `Send` if `T: Send`. Use `Arc` to share data between jobs.
Never use `Rc` in job closures — it is not `Send`.

---

## The `[this]` Capture Problem

In C++ you can capture `this` in a job lambda:
```cpp
builder.DispatchJob("name", [this, frame](const RunContext& ctx) {
    this->DoSomething(frame);
});
```

C++ trusts that `this` outlives the job. The compiler does not check.

In Rust, `self` is a borrow. A borrow cannot be `'static`. The compiler refuses:
```rust
// COMPILE ERROR — self is borrowed, not 'static
builder.dispatch_job("name", move |ctx| {
    self.do_something(); // self does not live long enough
});
```

### Solution: split into handle + inner Arc

```rust
// The public handle — cheap to clone, just bumps a refcount
pub struct RenderCommandHandler {
    inner: Arc<RenderCommandHandlerInner>,
}

// The actual data — lives as long as any Arc points to it
struct RenderCommandHandlerInner {
    flush_counter: Mutex<Counter>,
    draw_buffers_wait_counter: Mutex<Counter>,
    // ... everything else
}

// Counters that need reset() live behind interior mutability because the owner
// is shared through Arc. Keep lock scopes short: borrow the old counter for
// dispatch_wait, then lock again only to move the extracted replacement in.
impl RenderCommandHandler {
    pub fn render_scene(&self, jobs: &LeetJobSystem, frame: Arc<dyn IRenderFrame>) {
        let mut builder = jobs.create_builder(Priority::RenderPath);
        {
            let flush_counter = self.inner.flush_counter.lock().unwrap();
            builder.dispatch_wait(&flush_counter);
        }

        // Clone the Arc — cheap, just increments a counter
        let this = Arc::clone(&self.inner);
        let frame = Arc::clone(&frame);

        builder.dispatch_job("RenderScene/Flush", move |ctx| {
            // closure owns 'this' Arc and 'frame' Arc
            // both are guaranteed alive for the duration of this job
            this.do_something(&frame, ctx);
        });
    }
}
```

This makes the implicit C++ lifetime guarantee explicit in the type system.
`this` inside the closure keeps `RenderCommandHandlerInner` alive.
Even if the original `RenderCommandHandler` is dropped, the job still runs safely.

---

## Capture Translation Table

Every C++ job lambda capture has a direct Rust equivalent:

| C++ capture | What it actually is | Rust equivalent |
|---|---|---|
| `[frame]` | `TRenderPtr<IRenderFrame>` = ref-counted | `Arc::clone(&frame)`, move into closure |
| `[this]` | raw pointer, engine keeps alive | `Arc::clone(&self.inner)`, move into closure |
| `[scenePtr = x.Get()]` | ref-counted pointer by value | `Arc::clone(&scene_ptr)`, move into closure |
| `[value]` where value is int/bool | copied by value | `move`, value is copied into closure automatically |
| `[&local]` stack reference | **C++ bug waiting to happen** | does not compile in Rust — this is a feature |

The last row is important. If C++ code captures a stack reference in a job, it is
undefined behavior if the job outlives the stack frame. Rust refuses to compile it.
You will catch bugs by porting to Rust.

---

## Full Example: RenderScene Translation

```cpp
// C++ original
void CRenderCommandHandler::RenderScene(const TRenderPtr<IRenderFrame>& frame)
{
    job::Builder builder{ job::Priority::RenderPath };
    builder.DispatchWait(m_flushCounter);

    builder.DispatchJob("RenderScene/Flush", [frame](const job::RunContext& ctx) {
        SRenderFrameContext renderCtx(ctx, frame);
        GetRenderer()->RenderFrame(renderCtx);
    });

    m_flushCounter.Reset(builder.ExtractWaitCounter());
}
```

```rust
// Rust equivalent
impl RenderCommandHandler {
    pub fn render_scene(&self, jobs: &LeetJobSystem, frame: Arc<dyn IRenderFrame>) {
        let mut builder = jobs.create_builder(Priority::RenderPath);
        {
            let flush_counter = self.inner.flush_counter.lock().unwrap();
            builder.dispatch_wait(&flush_counter);
        }

        // Clone Arc for the closure — same as C++ ref-counted capture
        let frame = Arc::clone(&frame);
        builder.dispatch_job("RenderScene/Flush", move |ctx| {
            let render_ctx = RenderFrameContext::new(ctx, &frame);
            get_renderer().render_frame(render_ctx);
        });

        let wait_counter = builder.extract_wait_counter();
        self.inner.flush_counter.lock().unwrap().reset(wait_counter);
    }
}
```

The structure is the same dependency chain. Rust also makes the shared ownership
and counter mutation explicit: `Arc::clone` replaces implicit ref-counted
captures, and the stored counter is reset through interior mutability.

---

## Rules for Writing Job Closures

1. **Always use `move`** on job closures. Without `move`, Rust tries to borrow
   the captured variables, which is never `'static`.

2. **Clone Arcs before the closure, not inside it.**
   ```rust
   // GOOD
   let frame = Arc::clone(&frame);
   builder.dispatch_job("name", move |ctx| { use(&frame); });

   // BAD — trying to clone inside captures a reference to frame
   builder.dispatch_job("name", move |ctx| {
       let frame = Arc::clone(&frame); // frame was already moved — compile error
   });
   ```

3. **Use `Arc<Mutex<T>>` for data the job needs to mutate.**
   ```rust
   let shared = Arc::new(Mutex::new(my_data));
   let shared_clone = Arc::clone(&shared);
   builder.dispatch_job("name", move |ctx| {
       let mut guard = shared_clone.lock().unwrap();
       guard.do_mutation();
   });
   ```

4. **Use `Arc<AtomicU32>` (or similar) for counters and flags.**
   Prefer atomics over Mutex when only doing simple increments or flag sets.

5. **Never capture `&self` directly in a job closure.**
   Always go through `Arc::clone(&self.inner)`.
