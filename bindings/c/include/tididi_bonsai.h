/* tididi-bonsai: model counting by compiling a CNF into a Tree Decision Diagram.
 *
 * Copyright the tididi-bonsai authors. Licensed under the Apache License,
 * Version 2.0.
 *
 * Compare TIDIDI_BONSAI_ABI_VERSION with tididi_bonsai_abi_version() at run
 * time to catch a library built from a different ABI than this header.
 */

#ifndef TIDIDI_BONSAI_H
#define TIDIDI_BONSAI_H

/* Generated from bindings/c/src/lib.rs by cbindgen. Do not edit by hand; CI
 * checks that this file still matches the source. */

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/**
 * The version of the ABI this header declares. `tididi_bonsai_abi_version`
 * returns the version of the library that is actually loaded. It is raised
 * whenever a signature, a status code or an ownership rule changes in a way
 * that breaks a program built against the previous header.
 */
#define TIDIDI_BONSAI_ABI_VERSION 1

/**
 * What one count call produced: the answer, or the error that stopped the
 * run.
 *
 * Opaque. Read it with the `tididi_bonsai_answer_` functions and release it
 * with `tididi_bonsai_answer_free`. An answer does not change after the call
 * returns it, so several threads may read one at the same time; free it
 * once, after the last read.
 */
typedef struct tididi_bonsai_answer tididi_bonsai_answer;

/**
 * A status code: `TIDIDI_BONSAI_OK`, or one of the `TIDIDI_BONSAI_ERROR_`
 * values.
 */
typedef int32_t tididi_bonsai_code;

/**
 * One literal's weight, for `tididi_bonsai_count_clauses`: a `c p weight`
 * line.
 */
typedef struct tididi_bonsai_weight {
  /**
   * The literal: `3` for variable 3, `-3` for its negation.
   */
  int32_t literal;
  /**
   * The weight as NUL-terminated text, read as a `c p weight` line's is,
   * exactly: a decimal such as `0.3` or `-2.5e-3`, a fraction such as
   * `3/10`, or an integer.
   */
  const char *weight;
} tididi_bonsai_weight;

/**
 * The call succeeded.
 */
#define TIDIDI_BONSAI_OK 0

/**
 * The mode is unknown, contradicts the instance's header, or asks for an
 * unweighted count of an instance with `c p weight` lines.
 */
#define TIDIDI_BONSAI_ERROR_USAGE 1

/**
 * An environment setting the run reads is invalid.
 */
#define TIDIDI_BONSAI_ERROR_ENVIRONMENT 2

/**
 * A preprocessing or compile stage failed.
 */
#define TIDIDI_BONSAI_ERROR_STAGE 3

/**
 * The vtree construction cannot serve this run.
 */
#define TIDIDI_BONSAI_ERROR_VTREE_SPEC 4

/**
 * The input could not be opened or read, or does not describe a formula.
 */
#define TIDIDI_BONSAI_ERROR_INPUT 5

/**
 * The count could not be read off the compiled diagram.
 */
#define TIDIDI_BONSAI_ERROR_ARTIFACT 6

/**
 * The run gave up without a count, at the time limit or on a refused
 * allocation.
 */
#define TIDIDI_BONSAI_ERROR_INCOMPLETE 7

/**
 * An argument broke this header's contract, such as a null pointer with a
 * nonzero length.
 */
#define TIDIDI_BONSAI_ERROR_INVALID_ARGUMENT 8

/**
 * tididi-bonsai panicked. The panic stopped at this boundary and its message
 * is the error message; report it as a bug.
 */
#define TIDIDI_BONSAI_ERROR_PANIC 9

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

/**
 * The ABI version of the loaded library, which is the
 * `TIDIDI_BONSAI_ABI_VERSION` it was built with.
 */
uint32_t tididi_bonsai_abi_version(void);

/**
 * The version of tididi-bonsai inside the library, such as `0.1.0`, as a
 * NUL-terminated string. The string is static: the caller does not free it,
 * and it stays valid for as long as the library is loaded.
 */
const char *tididi_bonsai_version(void);

/**
 * Count the models of a DIMACS CNF given as text, as `tididi --mode MODE`
 * counts the same text in a file.
 *
 * `dimacs` points to `dimacs_len` bytes of DIMACS text, which need not end in
 * a NUL, with its `c t`, `c p show` and `c p weight` lines if it has them; it
 * may be null when `dimacs_len` is 0. The other arguments and the result are
 * those of `tididi_bonsai_count_file`.
 *
 * # Safety
 *
 * `dimacs` must be null or point to `dimacs_len` readable bytes that do not
 * change during the call; `mode` and `out` as for `tididi_bonsai_count_file`.
 */
tididi_bonsai_code tididi_bonsai_count(const uint8_t *dimacs,
                                       size_t dimacs_len,
                                       const char *mode,
                                       uint64_t time_limit_ms,
                                       uint64_t mem_limit_mb,
                                       struct tididi_bonsai_answer **out);

/**
 * Count the models of a CNF given as clauses of DIMACS integers, as `tididi
 * --mode MODE` counts the same formula written in a file.
 *
 * The formula has `num_vars` variables. `literals` points to `literals_len`
 * literals, the clauses one after another, each closed by a `0` as in the
 * clause lines of a DIMACS file: `3` is variable 3 and `-3` its negation. The
 * array ends with the `0` that closes its last clause, and a clause names each
 * variable at most once. `literals` may be null when `literals_len` is 0, a
 * formula with no clauses.
 *
 * `show` points to `show_len` variables, the `c p show` set a projected count
 * (`pmc`, `pwmc`) projects onto; null declares no show set, while a non-null
 * `show` with `show_len` 0 declares the empty one. `weights` points to
 * `weights_len` literal weights for a weighted count (`wmc`, `pwmc`), the
 * `c p weight` lines; a literal they leave out weighs 1, a literal given twice
 * takes its last weight, and `weights` may be null when `weights_len` is 0.
 * Clauses carry no `c t` line, so pass the mode: with a null `mode`, a show
 * set makes the count projected and anything else compiles.
 *
 * The other arguments and the result are those of
 * `tididi_bonsai_count_file`. A literal, shown variable or weight literal
 * that names no variable of the formula (0, `INT32_MIN`, or one past
 * `num_vars`), a clause that names a variable twice, and a weight that is not
 * a number return `TIDIDI_BONSAI_ERROR_INPUT`; `literals` not ending with 0
 * returns `TIDIDI_BONSAI_ERROR_INVALID_ARGUMENT`.
 *
 * # Safety
 *
 * `literals`, `show` and `weights` must each be null or point to as many
 * readable elements as its length says, each weight's `weight` must be null
 * or a NUL-terminated string, none of them may change during the call, and
 * `mode` and `out` are as for `tididi_bonsai_count_file`.
 */
tididi_bonsai_code tididi_bonsai_count_clauses(uint32_t num_vars,
                                               const int32_t *literals,
                                               size_t literals_len,
                                               const uint32_t *show,
                                               size_t show_len,
                                               const struct tididi_bonsai_weight *weights,
                                               size_t weights_len,
                                               const char *mode,
                                               uint64_t time_limit_ms,
                                               uint64_t mem_limit_mb,
                                               struct tididi_bonsai_answer **out);

/**
 * Count the models of the DIMACS CNF file at `path`, as `tididi --mode MODE
 * PATH` does.
 *
 * `path` is a NUL-terminated file name. `mode` is a NUL-terminated mode
 * token, one of `compile`, `mc`, `wmc`, `pmc` and `pwmc`, or null to take
 * the task the instance's `c t` and `c p show` lines declare, a plain
 * compile when they declare none; a mode given must agree with those lines.
 * `time_limit_ms` limits the whole run, and 0 sets no limit. `mem_limit_mb`
 * lowers the process's address-space limit (`RLIMIT_AS`) to that many MiB
 * while the run lasts, and 0 leaves it unchanged; the limit belongs to the
 * process, and the call puts it back when it returns. The library's `Limits`
 * documents both, and the user guide's `docs/limits.md` what a run that
 * reaches one does: the time limit is checked as the run goes, so a step in
 * progress can finish after it.
 *
 * Returns `TIDIDI_BONSAI_OK` or a `TIDIDI_BONSAI_ERROR_` code. Unless `out`
 * is null, `*out` receives a new answer whatever the code, and the code
 * equals `tididi_bonsai_answer_code(*out)`: the count on success, the error
 * message otherwise. The caller owns `*out` and releases it with
 * `tididi_bonsai_answer_free`. A null `out` returns
 * `TIDIDI_BONSAI_ERROR_INVALID_ARGUMENT` and produces no answer. Nothing
 * keeps a reference to the arguments after the call returns.
 *
 * The run prints its progress to stderr, as the binary does. Calls run one
 * at a time: a call made while another thread's call is running waits for it
 * to finish.
 *
 * # Safety
 *
 * `path` must be null or a NUL-terminated string, `mode` must be null or a
 * NUL-terminated string, neither may change during the call, and `out` must
 * be null or point to storage for one pointer.
 */
tididi_bonsai_code tididi_bonsai_count_file(const char *path,
                                            const char *mode,
                                            uint64_t time_limit_ms,
                                            uint64_t mem_limit_mb,
                                            struct tididi_bonsai_answer **out);

/**
 * Release an answer. Null is ignored.
 *
 * # Safety
 *
 * `answer` must be null or an answer a count call produced that has not been
 * freed. Every buffer borrowed from it becomes invalid.
 */
void tididi_bonsai_answer_free(struct tididi_bonsai_answer *answer);

/**
 * The code the count call returned with this answer: `TIDIDI_BONSAI_OK` or a
 * `TIDIDI_BONSAI_ERROR_` value. `TIDIDI_BONSAI_ERROR_INVALID_ARGUMENT` for a
 * null `answer`.
 *
 * # Safety
 *
 * `answer` must be null or a live answer from a count call.
 */
tididi_bonsai_code tididi_bonsai_answer_code(const struct tididi_bonsai_answer *answer);

/**
 * The mode the run resolved to, from the instance's header and the mode it
 * was given: `compile`, `mc`, `wmc`, `pmc` or `pwmc`.
 *
 * Writes the token's length to `*len` when `len` is not null. The bytes
 * belong to `answer`: they stay valid and unchanged until
 * `tididi_bonsai_answer_free(answer)`, and are followed by a NUL that `*len`
 * does not count. Null, with `*len` set to 0, for a failed or null `answer`.
 *
 * # Safety
 *
 * `answer` must be null or a live answer from a count call, and `len` must be
 * null or point to storage for one `size_t`.
 */
const char *tididi_bonsai_answer_mode(const struct tididi_bonsai_answer *answer, size_t *len);

/**
 * Whether the formula has a model. This is the run's verdict, not read off
 * the count: a zero weight, or weights that cancel, make the weighted count
 * of a satisfiable formula zero. False for a failed or null `answer`.
 *
 * # Safety
 *
 * `answer` must be null or a live answer from a count call.
 */
bool tididi_bonsai_answer_satisfiable(const struct tididi_bonsai_answer *answer);

/**
 * The count, spelled as the binary's answer line spells it: a decimal
 * integer for `compile`, `mc` and `pmc`; an exact fraction in lowest terms,
 * `<numerator>/<denominator>`, for `pwmc`; and for `wmc`, which counts in a
 * bounded-precision log domain, `<mantissa>e<exponent>` with the mantissa in
 * [1, 10) and signed when negative, or `0`. The count is over every variable
 * the formula declares, or over the `c p show` variables on the projected
 * modes.
 *
 * Writes the text's length to `*len` when `len` is not null. The bytes belong
 * to `answer`: they stay valid and unchanged until
 * `tididi_bonsai_answer_free(answer)`, and are followed by a NUL that `*len`
 * does not count. Null, with `*len` set to 0, for a failed or null `answer`.
 *
 * # Safety
 *
 * `answer` must be null or a live answer from a count call, and `len` must be
 * null or point to storage for one `size_t`.
 */
const char *tididi_bonsai_answer_count(const struct tididi_bonsai_answer *answer, size_t *len);

/**
 * The count as the nearest `double`, for printing or plotting: lossy, since
 * a large count rounds to a `double`'s precision and a magnitude past its
 * range is infinite. `tididi_bonsai_answer_count` spells the exact count.
 * NaN for a failed or null `answer`.
 *
 * # Safety
 *
 * `answer` must be null or a live answer from a count call.
 */
double tididi_bonsai_answer_count_f64(const struct tididi_bonsai_answer *answer);

/**
 * The run's record as one JSON object, the one `tididi --stats` writes:
 * the mode, the count, sizes, timings and which stage answered. The user
 * guide's `docs/stats.md` defines each field.
 *
 * Writes the text's length to `*len` when `len` is not null. The bytes belong
 * to `answer`: they stay valid and unchanged until
 * `tididi_bonsai_answer_free(answer)`, and are followed by a NUL that `*len`
 * does not count. Null, with `*len` set to 0, for a failed or null `answer`.
 *
 * # Safety
 *
 * `answer` must be null or a live answer from a count call, and `len` must be
 * null or point to storage for one `size_t`.
 */
const char *tididi_bonsai_answer_stats_json(const struct tididi_bonsai_answer *answer, size_t *len);

/**
 * The error message: UTF-8 text saying what went wrong, as the binary prints
 * it on stderr. It names the input line, mode or setting at fault where there
 * is one. A run that gave up (`TIDIDI_BONSAI_ERROR_INCOMPLETE`) says what
 * ended it; what stopped each stage went to stderr as the run went.
 *
 * Writes the message's length to `*len` when `len` is not null. The bytes
 * belong to `answer`: they stay valid and unchanged until
 * `tididi_bonsai_answer_free(answer)`, and are followed by a NUL that `*len`
 * does not count. Null, with `*len` set to 0, for a successful or null
 * `answer`.
 *
 * # Safety
 *
 * `answer` must be null or a live answer from a count call, and `len` must be
 * null or point to storage for one `size_t`.
 */
const char *tididi_bonsai_answer_error_message(const struct tididi_bonsai_answer *answer,
                                               size_t *len);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* TIDIDI_BONSAI_H */
