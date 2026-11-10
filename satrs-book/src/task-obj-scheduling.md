# Task and Object Scheduling

For the implementation of on-board software, the chances are very high that you will use some
sort of (real-time) operating system or runtime to help with the execution of concurrent tasks.

In this chapter, we will explore some common scheduling patterns used by space systems and how we
can map them to those mechanisms when using Rust.

Task scheduling controls when and how often tasks are performed in your system.
It is a central aspect when it comes to meeting time constraints of your system.
All of this becomes clearer with an example.

## Example - AOCS

You might have an attitude and orbit control cycle which needs to run every 400 milliseconds due to
system requirements. Let's assume a simplified AOCS which only relies on measuring a magnetic
field using magnetometers (MGMs) and then doing attitude control using a magnetorquer (MTQ). You
usually want to measure the magnetic field of the earth for attitude measurements while the MTQ is
off. However, the torquing period of the MTQ should usually be maximized. Furthermore, you have
some sort of attitude control algorithm that takes a target attitude and current attitude
information as input and has the corresponding actuator commands as output. Usually, you want to
run this algorithm between fetching the newest MGM data from sensors and actuating with the most
recent actuator commands. The algorithm should also run at the same relative time inside
the periodic scheduling window because this is a favorable property for control algorithms.

So we already have a system with the following order and constraints, which need to be enforced:

1. Read MGM sensors, ensuring that this measurement is done without any active MTQ output.
2. Perform ACS control algorithm.
3. Drive actuators for a fixed period, switch them off after that.

As the next step, we will classify objects and then talk about possible solutions to enforce
those timing constraints.

## Object scheduling

We like to model individual building blocks of our software using objects. Rust does not offer
inheritance, but it still allows building object-oriented software. Common examples:

- `MgmHandler` object for interaction with a physical MGM device.
- `AcsController` object for execution of attitude control algorithms.
- `SatelliteSystem` object for management of the top-level mode tree of the system.
- `PayloadHandler` object for controlling a payload and writing large amounts of generated data
  to the disk.
- `CoreController` object for managing the CPU itself, e.g. requesting reboots or reading
  internal sensors.

However, how do we actually run these objects? There are various approaches here. We could
have a `run` method which just never returns and then gets called in some OS thread/task.
We could also have some `operation` or `poll` method which might be called multiple times by the OS
thread/task. This is a very common pattern in control systems where control operations often
need to be performed periodically without user input to meet requirements. Sometimes, your system
object might also have temporal requirements, for example running at a specific time relative to
other objects.

Depending on what kind of object you have, different approaches can be viable.

Three core object groups were identified:

1. Objects which need to be polled periodically and which do not have timing constraints with other
   objects. Example: The `CoreController` listed above.
2. Objects which need to be polled periodically but which have timing constraints with other
   objects. Example: The `MgmHandler` or `AcsController` object listed above.
3. Objects which might be polled but which might also consume large amounts of resources for
   longer periods. Example: `PayloadHandler` listed above.

## Mapping objects to operating system tasks

Operating system threads and tasks can be expensive. Depending on the system, each task might
have a static or dynamic cost. Furthermore, context switches between different tasks can be
expensive as well. Generally, it is preferable to minimize the number of tasks if possible.

For the object classification above, we can do the following mapping to operating system
tasks/threads:

1. You can map 1-N of these objects onto one OS thread with a fixed or variable scheduling
   frequency. Grouping objects allows minimizing the number of OS threads.
2. You can group the scheduling of these objects into one OS task. You can then poll the
   individual objects in the correct order and then use (deadline-aware) delays to meet the timing
   constraints of your system.
3. You can map one object to one OS task/thread. This is important so that an expensive or
   long-running operation does not block another object.

### Using `std` library `thread`s

This is an excerpt from the `std` example code for object group 1. It can easily be adapted to
object group 3 by only executing one object.

```rust,ignore
{{#include ../../examples/example-std/src/main.rs:controller_thread}}
```

This is an excerpt from the `std` example code for object group 2:

```rust,ignore
{{#include ../../examples/example-std/src/main.rs:aocs_thread}}
```

The join handle (`jh_*`) and the `KILL_SIGNAL` are components used for a graceful shutdown or
reboot of the software and the system.

The [AOCS Scheduling](./aocs-scheduling.md) chapter shows some timing diagrams of the AOCS, which
are useful for visualizing the timing constraints of the system.

### Using `async` API on `embassy`

Embassy tasks are `async` functions which are run by an executor.
This is an excerpt from the embedded example code. It polls the PCDU handler with a fixed timing.
Like in the `std` example, more objects can be polled in the same loop for object group 1.
Here, we are also polling the same object multiple times during a task cycle.

```rust,ignore
{{#include ../../examples/embedded/shared-embedded/src/pcdu.rs:pcdu_task}}
```

The task is declared with the `embassy_executor::task` macro and spawned once at startup:

```rust,ignore
{{#include ../../examples/embedded/stm32h7-nucleo-embassy/src/main.rs:pcdu_task_wrapper}}
```

```rust,ignore
{{#include ../../examples/embedded/stm32h7-nucleo-embassy/src/main.rs:spawn_pcdu_task}}
```

The excerpt already runs only one object, which is the mapping for object group 3. However, the
executor schedules its tasks cooperatively. A task only gives up the CPU at an `.await`, so a long
computation without `.await` points still blocks all other tasks of the same executor. Such
objects need their own executor with a higher priority, for example an `InterruptExecutor`.
