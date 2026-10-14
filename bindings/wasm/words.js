// The words the page puts to tididi-bonsai's tokens, and small formatters.

// What the answer line calls each mode's count (`docs/tasks.md`).
export const ANSWER_LABELS = {
  compile: "model count",
  mc: "model count",
  wmc: "weighted count",
  pmc: "projected model count",
  pwmc: "projected weighted count",
};

export const MODE_NAMES = {
  compile: "compile, then count the diagram's models",
  mc: "model counting",
  wmc: "weighted model counting, in double precision",
  pmc: "projected model counting",
  pwmc: "projected weighted model counting, exact",
};

// The stage that answered a model count or a compile, the record's
// `answered_by`.
export const ANSWERED_BY = {
  compile: "the compile",
  preprocessing: "preprocessing, with nothing left to compile",
  "stall-recompile": "a second compile of the reduced formula, on another vtree",
  "support-enumeration": "enumerating the reduced formula's independent support",
  "no-sbva-retry": "a compile reduced again without SBVA",
  "unreduced-retry": "a compile of the formula Arjun was given",
  "monotone-reduction": "a compile of the reduction Arjun skipped on a monotone formula",
  "shannon-split": "splitting the formula on variables and counting the pieces",
  "bisect-retry": "a second compile, on a hypergraph bisection, after the first ran out of memory",
};

// An error's kind, the record's `exit_reason`, and the worker's own.
export const ERROR_KINDS = {
  usage: "a setting was refused",
  environment: "a setting in the environment was refused",
  stage: "a stage failed",
  "vtree-spec": "the vtree construction was refused",
  input: "the input was refused",
  artifact: "an artifact could not be produced",
  incomplete: "the run gave up without a count",
  panic: "tididi-bonsai panicked",
  load: "tididi-bonsai did not load",
  worker: "the worker failed",
};

export const plural = (count, word) => `${count.toLocaleString("en-US")} ${count === 1 ? word : `${word}s`}`;

export function megabytes(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1048576).toFixed(1)} MB`;
}

export function duration(ms) {
  if (ms < 1) return "under 1 ms";
  if (ms < 10000) return `${Math.round(ms)} ms`;
  return `${(ms / 1000).toFixed(1)} s`;
}
