// Reproduction of the fault counter figure from B. Bätz, "Design and Implementation of a
// Framework for Spacecraft Flight Software", Ph.D. Thesis, University of Stuttgart, 2020, p. 173.
#set page(width: auto, height: auto, margin: 0.5cm)
#import "@preview/cetz:0.4.2"

#cetz.canvas({
  import cetz.draw: *

  // axes
  line((-0.2, 0), (11, 0), mark: (end: "stealth", fill: black), stroke: 1.2pt)
  content((11, -0.2), [time], anchor: "north")
  line((0, -0.2), (0, 3.2), mark: (end: "stealth", fill: black), stroke: 1.2pt)
  content((-0.2, 3.2), [count], anchor: "east")

  // threshold
  line((0, 2.5), (10.5, 2.5), stroke: (dash: "dashed"))
  content((5.25, 2.5), [threshold], anchor: "south", padding: 0.1)

  // counter (step curve)
  line(
    (0, 0), (1.5, 0), (1.5, 0.5), (2, 0.5), (2, 1), (3, 1),
    (3, 1.5), (5, 1.5), (5, 1), (7, 1), (7, 0.5), (8, 0.5),
    (8, 1), (8.3, 1), (8.3, 1.5), (8.8, 1.5), (8.8, 2),
    (9.5, 2), (9.5, 2.7),
    stroke: 2pt,
  )

  // fault markers
  for (x, y) in ((1.5, 0.5), (2, 1), (3, 1.5), (8, 1), (8.3, 1.5), (8.8, 2)) {
    content((x - 0.1, y), [!], anchor: "south-east")
  }
  content((9.5, 3), [↯])

  // Δt intervals
  for x in (3, 5, 7) { line((x, 0), (x, 1)) }
  content((4, -0.1), $Delta t$, anchor: "north")
  content((6, -0.1), $Delta t$, anchor: "north")
})
