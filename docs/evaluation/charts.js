// The page's two plots, drawn as inline SVG at the width of the box they sit
// in: the cactus plot and the head-to-head plot. Each box also holds a
// tooltip (`.tip`) and a live region (`.live`), which read out the point
// nearest the pointer, or the point the arrow keys step to on a focused plot.

const SVG = "http://www.w3.org/2000/svg";

// How near the pointer, in pixels, a point has to be to be read out.
const REACH = 24;

function svgNode(name, attributes = {}, text) {
  const node = document.createElementNS(SVG, name);
  for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, value);
  if (text !== undefined) node.textContent = text;
  return node;
}

// A time as the page writes it.
export const seconds = (time) => `${time.toFixed(2)} s`;

// A logarithmic map of [low, high] seconds onto [from, to] pixels.
function logScale(low, high, from, to) {
  const start = Math.log(low);
  const span = Math.log(high) - start;
  return (time) => from + ((to - from) * (Math.log(Math.max(time, low)) - start)) / span;
}

// The ticks of a time axis `length` pixels long: 1 and 3 times each power of
// ten from `low`, or the powers alone when those would sit closer than
// `spacing` pixels, then the time limit `high`, without a tick that would
// crowd it.
function timeTicks(low, high, length, spacing) {
  const ticks = (steps) => {
    const values = [];
    for (let power = low; power < high; power *= 10) {
      for (const step of steps) values.push(Number((power * step).toPrecision(2)));
    }
    return [...values.filter((value) => value < high / 1.4), high];
  };
  const fine = ticks([1, 3]);
  return length / fine.length >= spacing ? fine : ticks([1]);
}

// A count axis from 0 to at least `most`, in steps of 1, 2 or 5 times a power
// of ten, at most one per `spacing` pixels.
function countTicks(most, length, spacing) {
  const room = Math.max(1, Math.floor(length / spacing));
  let step;
  for (let power = 1; step === undefined; power *= 10) {
    step = [1, 2, 5].map((m) => m * power).find((s) => Math.ceil(most / s) <= room);
  }
  const end = Math.max(1, Math.ceil(most / step)) * step;
  return { end, ticks: Array.from({ length: end / step + 1 }, (_, i) => i * step) };
}

// Sizes the plot and starts it afresh with its title and description.
function begin(svg, width, height, title, description) {
  svg.setAttribute("width", width);
  svg.setAttribute("height", height);
  svg.setAttribute("viewBox", `0 0 ${width} ${height}`);
  svg.replaceChildren(svgNode("title", {}, title), svgNode("desc", {}, description));
}

// The grid, the tick labels and the axis titles around `plot`, the plotting
// area's edges in pixels; each tick is its position and its label.
function frame(svg, plot, xTicks, yTicks, xTitle, yTitle) {
  for (const { at, label } of yTicks) {
    svg.append(
      svgNode("line", { class: "grid", x1: plot.left, x2: plot.right, y1: at, y2: at }),
      svgNode("text", { class: "tick", x: plot.left - 8, y: at + 4, "text-anchor": "end" }, label),
    );
  }
  for (const { at, label } of xTicks) {
    svg.append(
      svgNode("line", { class: "grid", x1: at, x2: at, y1: plot.top, y2: plot.bottom }),
      svgNode("text", { class: "tick", x: at, y: plot.bottom + 18, "text-anchor": "middle" }, label),
    );
  }
  svg.append(
    svgNode("path", { class: "axis", d: `M${plot.left} ${plot.top}V${plot.bottom}H${plot.right}` }),
    svgNode("text", { class: "axis-title", x: (plot.left + plot.right) / 2, y: plot.bottom + 38, "text-anchor": "middle" }, xTitle),
    svgNode("text", {
      class: "axis-title",
      transform: `translate(14 ${(plot.top + plot.bottom) / 2}) rotate(-90)`,
      "text-anchor": "middle",
    }, yTitle),
  );
}

// Reads out `points`, each its position, its colour and the lines its tooltip
// shows (a line with a colour is keyed by it), in the plot's box.
function readout(svg, points) {
  const box = svg.parentElement;
  const tip = box.querySelector(".tip");
  const live = box.querySelector(".live");
  const ring = svgNode("circle", { class: "ring", r: 7, visibility: "hidden" });
  svg.append(ring);
  let current = -1;

  const hide = () => {
    current = -1;
    ring.setAttribute("visibility", "hidden");
    tip.hidden = true;
  };
  const show = (index) => {
    current = index;
    const point = points[index];
    ring.setAttribute("cx", point.x);
    ring.setAttribute("cy", point.y);
    ring.style.stroke = point.colour;
    ring.setAttribute("visibility", "visible");
    tip.replaceChildren(...point.lines.map((line) => {
      const row = document.createElement(line.strong ? "strong" : "span");
      if (line.colour) {
        const key = document.createElement("i");
        key.className = "key";
        key.style.background = line.colour;
        row.append(key);
      }
      row.append(line.text);
      return row;
    }));
    tip.hidden = false;
    const [drawing, around] = [svg.getBoundingClientRect(), box.getBoundingClientRect()];
    const left = drawing.left - around.left + point.x;
    const top = drawing.top - around.top + point.y;
    const right = left + 14 + tip.offsetWidth <= box.clientWidth;
    tip.style.left = `${right ? left + 14 : Math.max(0, left - 14 - tip.offsetWidth)}px`;
    tip.style.top = `${top - tip.offsetHeight - 10 >= 0 ? top - tip.offsetHeight - 10 : top + 14}px`;
  };

  svg.onpointermove = (event) => {
    const bounds = svg.getBoundingClientRect();
    const x = event.clientX - bounds.left;
    const y = event.clientY - bounds.top;
    let nearest = -1;
    let distance = REACH * REACH;
    points.forEach((point, index) => {
      const d = (point.x - x) ** 2 + (point.y - y) ** 2;
      if (d < distance) [nearest, distance] = [index, d];
    });
    if (nearest < 0) hide();
    else if (nearest !== current) show(nearest);
  };
  svg.onpointerdown = svg.onpointermove;
  svg.onpointerleave = hide;
  svg.onblur = hide;
  svg.onkeydown = (event) => {
    const step = { ArrowRight: 1, ArrowUp: 1, ArrowLeft: -1, ArrowDown: -1 }[event.key];
    if (step === undefined || points.length === 0) return;
    event.preventDefault();
    show(current < 0 ? 0 : (current + step + points.length) % points.length);
    live.textContent = points[current].lines.map((line) => line.text).join(", ");
  };
  live.textContent = "";
  hide();
}

// The cactus plot: each curve's times, fastest first, against how many
// instances the solver finished within each, up to `most`, the largest count
// any solver reaches, on a time axis from `low` up to the time limit. Ours
// is drawn last, on top.
export function drawCactus(svg, { width, curves, most, total, low, timeout, title }) {
  const height = Math.round(Math.min(560, Math.max(260, width * 0.78)));
  const plot = { left: 60, right: width - 16, top: 12, bottom: height - 46 };
  const { end, ticks } = countTicks(most, plot.right - plot.left, 64);
  const x = (count) => plot.left + ((plot.right - plot.left) * count) / end;
  const y = logScale(low, timeout, plot.bottom, plot.top);
  const solved = curves.map((curve) => `${curve.name} solves ${curve.times.length}`).join(", ");
  begin(svg, width, height, title, `Cactus plot of ${total} instances. ${solved || "No solver is shown"}.`);
  frame(
    svg,
    plot,
    ticks.map((count) => ({ at: x(count), label: String(count) })),
    timeTicks(low, timeout, plot.bottom - plot.top, 26).map((time) => ({ at: y(time), label: `${time} s` })),
    `instances solved, of ${total}`,
    "time per instance",
  );

  const points = [];
  for (const curve of [...curves].sort((x, y) => x.ours - y.ours)) {
    const at = curve.times.map((time, i) => ({ x: x(i + 1), y: y(time) }));
    const style = `stroke: ${curve.colour}`;
    svg.append(svgNode("polyline", {
      class: curve.ours ? "curve ours" : "curve",
      points: at.map((p) => `${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(" "),
      style,
    }));
    if (at.length > 0) {
      svg.append(svgNode("circle", { class: "end", cx: at.at(-1).x, cy: at.at(-1).y, r: curve.ours ? 3.5 : 2.5, style: `fill: ${curve.colour}` }));
    }
    curve.times.forEach((time, i) => points.push({
      ...at[i],
      colour: curve.colour,
      lines: [{ text: `${i + 1} instances within ${seconds(time)}`, strong: true }, { text: curve.name, colour: curve.colour }],
    }));
  }
  readout(svg, points);
}

// The head-to-head plot of solver a (across) against solver b (up): a point
// per instance either solved, at the two times, coloured by the solver that
// finished first, with an instance a solver did not solve in the band at the
// end of its axis, inside the band's label. The diagonal is where the two
// take the same time.
export function drawScatter(svg, { width, a, b, instances, low, timeout, tie, title }) {
  const size = Math.min(width, 560);
  const plot = { left: 60, right: size - 12, top: 12, bottom: size - 46 };
  const band = 30;
  const x = logScale(low, timeout, plot.left, plot.right - band - 6);
  const y = logScale(low, timeout, plot.bottom, plot.top + band + 6);
  const outX = plot.right - 20;
  const outY = plot.top + 20;
  const ticks = timeTicks(low, timeout, plot.right - band - plot.left, 52);
  const shown = instances.filter((instance) => instance.times.get(a.name) !== null || instance.times.get(b.name) !== null);
  begin(svg, size, size, title, `Scatter plot of the ${shown.length} instances ${a.name} or ${b.name} solved.`);
  frame(
    svg,
    plot,
    ticks.map((time) => ({ at: x(time), label: `${time} s` })),
    ticks.map((time) => ({ at: y(time), label: `${time} s` })),
    `${a.name}, time per instance`,
    `${b.name}, time per instance`,
  );
  const middle = (plot.top + plot.bottom) / 2;
  svg.append(
    svgNode("rect", { class: "band", x: plot.left, y: plot.top, width: plot.right - plot.left, height: band }),
    svgNode("rect", { class: "band", x: plot.right - band, y: plot.top, width: band, height: plot.bottom - plot.top }),
    svgNode("text", { class: "band-label", x: plot.left + 6, y: plot.top + 10 }, `not solved by ${b.name}`),
    svgNode("text", {
      class: "band-label",
      transform: `translate(${plot.right - 3} ${middle}) rotate(-90)`,
      "text-anchor": "middle",
    }, `not solved by ${a.name}`),
    svgNode("line", { class: "diagonal", x1: x(low), y1: y(low), x2: x(timeout), y2: y(timeout) }),
    svgNode("text", { class: "region", x: plot.left + 8, y: plot.top + band + 22 }, `${a.name} first`),
    svgNode("text", { class: "region", x: plot.right - band - 10, y: plot.bottom - 10, "text-anchor": "end" }, `${b.name} first`),
  );

  const time = (t) => (t === null ? "not solved" : seconds(t));
  const points = shown.map((instance) => {
    const ta = instance.times.get(a.name);
    const tb = instance.times.get(b.name);
    const colour = ta === tb ? tie : tb === null || (ta !== null && ta < tb) ? a.colour : b.colour;
    const point = { x: ta === null ? outX : x(ta), y: tb === null ? outY : y(tb), colour };
    svg.append(svgNode("circle", { class: "dot", cx: point.x, cy: point.y, r: 3.5, style: `fill: ${colour}` }));
    point.lines = [
      { text: instance.name, strong: true },
      { text: instance.set },
      { text: `${a.name}: ${time(ta)}`, colour: a.colour },
      { text: `${b.name}: ${time(tb)}`, colour: b.colour },
    ];
    return point;
  });
  readout(svg, points);
}
