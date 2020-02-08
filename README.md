# Scoped Spawn

[![crates.io](https://img.shields.io/crates/v/scoped_spawn)](https://crates.io/crates/scoped_spawn)
[![Pipeline Status](https://gitlab.com/upsp/scoped_spawn/badges/master/pipeline.svg)](https://gitlab.com/upsp/scoped_spawn/-/commits/master)

Full structured concurrency for asynchronous programming.

## Structured Concurrency

In structured concurrency, each asynchronous task only runs within a certain
scope. This scope is created by the task's parent and cannot exceed the
parent's scope.

At each point in time, tasks created within structured concurrency form a tree.
If a node is alive, all the nodes on its path to the root are also alive. In
other words, no node can outlive its parent.

This library provides a strong guarantee of structured concurrency: a child
task must completely exit and release all resources (except for the resources
to notify its parent of its termination, which is handled by this library)
before we consider it terminated.

## API Overview

This library provides the `ScopedSpawn` trait from which you can spawn new
tasks. The spawned tasks become children of the current task and will be
terminated when the current task begins to terminate. The API also provides
methods with which you could terminate a child task earlier.

Termination initiated outside the task to be terminated is also called
cancellation.

The `ScopedSpawn` trait is implemented by `ScopedSpawner`. To create a
`ScopedSpawner`, pass it an object that implements `Spawn`, which you can
trivially implement for all known executors.

Any code that wishes to accept a spawner should accept the `ScopedSpawn` trait
instead of `ScopedSpawner`.

## Termination of a Task

The termination process has several phases.

1. Termination begins upon the completion of the task's future or the receipt
   of cancel signal from the parent, whichever comes first.
2. The task's future is immediately dropped.
3. The task in turn sends cancel signals to its children.
4. The task asynchronously waits for its children to terminate.
5. The `done` function is called in the task. For details see the documentation
   for `ScopedSpawn`.
6. Finally, the task signals its termination to its parent through the "done"
   signal. For details see the documentation for `ParentSignals` and
   `ChildSignals`.

## Low-level API

A low-level `remote_scope` API is also provided. It gives you everything you
need to spawn a task but does not do the actual spawning.

## Example

The following example demonstrates `ScopedSpawn` when using Tokio.

```rust
#[tokio::main]
async fn main() {
    use scoped_spawn::{ScopedSpawn, ScopedSpawner};

    let spawn = TokioDefaultSpawner::new();
    let spawn = ScopedSpawner::new(spawn);
    let signal = spawn
        .spawn_with_signal(
            |spawn| async {
                // Here `spawn` is the child's spawner. Do not give it to anyone else!
                // And do not try to use the parent's spawner because it would break structured
                // concurrency.

                // We could spawn nested children here, but for the demo we don't.
                drop(spawn);

                eprintln!("I'm alive!");
                tokio::time::delay_for(std::time::Duration::from_secs(2)).await;
                eprintln!("I'm still alive!"); // Nope.
            },
            || (),
        )
        .unwrap();

    tokio::time::delay_for(std::time::Duration::from_secs(1)).await;

    drop(signal.cancel_sender); // Cancel the task by dropping.
    signal.done_receiver.await; // Do this if you want to wait.
                                // When the await returns, the future of the spawned task is
                                // guaranteed dropped.
    eprintln!("Task terminated.");
}

// Just some chores to turn the Tokio spawner into a `Spawn`.
#[derive(Clone)]
struct TokioDefaultSpawner {}

impl TokioDefaultSpawner {
    fn new() -> Self {
        Self {}
    }
}

impl futures::task::Spawn for TokioDefaultSpawner {
    fn spawn_obj(
        &self,
        future: futures::future::FutureObj<'static, ()>,
    ) -> Result<(), futures::task::SpawnError> {
        tokio::spawn(future);
        Ok(())
    }

    fn status(&self) -> Result<(), futures::task::SpawnError> {
        Ok(())
    }
}
```
