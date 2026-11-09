# Working with Actions

Space systems generally need to be commanded regularly. This can include commands periodically
required to ensure a healthy system, or commands to reach the mission goals.

These commands can be modelled using the concept of Actions. If you have not read the 
[Packet Format and TMTC Modelling](./tmtc-modelling.md) chapter yet, it is recommended to read it first.

For a low number of actions, it is recommended to add the actions as `enum` variants of your
`Request` type. For a higher number of actions, you can create a dedicated `ActionRequest`
structure.
