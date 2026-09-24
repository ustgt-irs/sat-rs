# Modes

Modes are an extremely useful concept to model complex systems. They allow simplified
system reasoning for both system operators and OBSW developers. They also provide a way to alter
the behaviour of a component and also provide observability of a system. A few examples of how to
model the mode of different components within a space system with modes will be given.

## Physical device component with modes

The following simple mode scheme with the following three modes

- `OFF`
- `ON`
- `NORMAL`

can be applied to a large number of simpler device controllers of a remote system, for example
sensors.

1. `OFF` means that a device is physically switched off, and the corresponding software component
does not poll the device regularly.
2. `ON` means that a device is physically switched on, but the device is not polled periodically.
3. `NORMAL` means that a device is powered on and polled periodically.

If a device is `OFF`, the device handler will deny commands which include physical communication
with the connected devices. In `NORMAL` mode, it will autonomously perform periodic polling
of a connected physical device in addition to handling remote commands by the operator.
Using these three basic modes, there are two important transitions which need to be taken care of
for the majority of devices:

1. `OFF` to `ON` or `NORMAL`: The device first needs to be powered on. After that, the
   device initial startup configuration must be performed.
2. `NORMAL` or `ON` to `OFF`: Any important shutdown configuration or handling must be performed
   before powering off the device.

## Controller components with modes

Controller components are not modelling physical devices, but a mode scheme is still the best
way to model most of these components.

For example, a hypothetical attitude controller might have the following modes:

- `SAFE`
- `TARGET IDLE`
- `TARGET POINTING GROUND`
- `TARGET POINTING NADIR`

We can also introduce the concept of submodes: The `SAFE` mode can for example have a
`DEFAULT` submode and a `DETUMBLE` submode.

## Achieving system observability with modes

If a system component has a mode in some shape or form, this mode should be observable. This means
that the operator can also retrieve the mode for a particular component. This is especially
important if these components can change their mode autonomously.

If a component is able to change its mode autonomously, this is also something which is relevant
information for the operator or for other software components. This means that a component
should also be able to announce its mode.

This concept becomes especially important when applying the mode concept on the whole
system level. This will also be explained in detail in a dedicated chapter, but the basic idea
is to model the whole system as a tree where each node has a mode. A new capability is added now:
A component can announce its mode recursively. This means that the component will announce its
own mode first before announcing the mode of all its children. Using a scheme like this, the mode
of the whole system can be retrieved using only one command. The same concept can also be used
for commanding the whole system, which will be explained in more detail in the dedicated systems
modelling chapter.

In summary, a component which has modes has to expose the following 4 capabilities:

1. Set a mode
2. Read the mode
3. Announce the mode
4. Announce the mode recursively

# Health

Health is an important concept for systems and components which might fail.
Oftentimes, the health is tied to the mode of a system component in some shape or form, and
determines whether a system component is usable. Health is also an extremely useful concept
to simplify the Fault Detection, Isolation and Recovery (FDIR) concept of a system.

The following health states are based on the ones used inside the FSFW and are enough to model most
use-cases:

- `HEALTHY`
- `FAULTY`
- `NEEDS RECOVERY`
- `EXTERNAL CONTROL`

1. `HEALTHY` means that a component is working nominally, and can perform its task without any issues.
2. `FAULTY` means that a component does not work properly. This might also impact other system
components, so the passivation and isolation of that component is desirable for FDIR purposes.
3. `NEEDS RECOVERY` is used to attempt a recovery of a component. For example, a simple sensor
could be power-cycled if there were multiple communication issues recently.
4. `EXTERNAL CONTROL` is used to isolate an individual component from the rest of the system. For
   example, an operator might be interested in testing a component in isolation, and the interference
   of the system is not desired. In that case, the `EXTERNAL CONTROL` health state might be used
   to prevent mode commands from the system while allowing external mode commands.
