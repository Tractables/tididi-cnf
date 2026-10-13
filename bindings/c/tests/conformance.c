/* Tests the C API through its header: counts equal to the tididi
 * executable's on the instances bindings/parity.txt lists, read from a file,
 * from text and as clauses, counts of small formulas, the error codes and
 * messages, the pointer and length contracts, and repeated and concurrent
 * calls.
 *
 *   conformance TIDIDI_EXECUTABLE REPOSITORY_ROOT
 *
 * TIDIDI_EXECUTABLE is the tididi binary built from the same source. The
 * failures and the summary go to stdout; stderr carries the runs' progress. */

#define _XOPEN_SOURCE 700

#include <math.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "tididi_bonsai.h"

static int failures = 0;

#define CHECK(cond, ...)                                                       \
  do {                                                                         \
    if (!(cond)) {                                                             \
      printf("FAIL %s:%d: ", __FILE__, __LINE__);                              \
      printf(__VA_ARGS__);                                                     \
      putchar('\n');                                                           \
      failures++;                                                              \
    }                                                                          \
  } while (0)

#define LINE_SIZE 4096
#define MAX_LITERALS 65536
#define MAX_NAMED 1024

/* (x1 or x2) and (not x1 or x3) and (not x2 or not x3): two models. */
static const char TWO_MODELS[] = "p cnf 3 3\n1 2 0\n-1 3 0\n-2 -3 0\n";
static const char TWO_MODELS_MC[] = "c t mc\np cnf 3 3\n1 2 0\n-1 3 0\n-2 -3 0\n";
static const char REFUTED[] = "p cnf 1 2\n1 0\n-1 0\n";
/* x1 or x2, with x1 weighing 3/10 true and 7/10 false and x2 1/2 either
 * way: 3/20 + 3/20 + 7/20 = 13/20. */
static const char WEIGHTED[] =
    "c t wmc\np cnf 2 1\nc p weight 1 3/10 0\nc p weight -1 7/10 0\n"
    "c p weight 2 1/2 0\nc p weight -2 1/2 0\n1 2 0\n";
static const char MALFORMED[] = "p cnf 2 1\n1 x 0\n";
/* TWO_MODELS as clauses. */
static const int32_t TWO_MODELS_LITERALS[] = {1, 2, 0, -1, 3, 0, -2, -3, 0};
#define TWO_MODELS_LEN (sizeof TWO_MODELS_LITERALS / sizeof *TWO_MODELS_LITERALS)

/* A formula as tididi_bonsai_count_clauses takes it. */
typedef struct {
  uint32_t num_vars;
  int32_t literals[MAX_LITERALS];
  size_t literals_len;
  int has_show;
  uint32_t show[MAX_NAMED];
  size_t show_len;
  tididi_bonsai_weight weights[MAX_NAMED];
  char weight_text[MAX_NAMED][64];
  size_t weights_len;
} clauses;

static const char *or_null(const char *text) { return text ? text : "(null)"; }

/* Whether `value` is `expected` up to the rounding of a log-domain count. */
static int near(double value, double expected) {
  double difference = value - expected;
  return difference < 1e-9 && difference > -1e-9;
}

/* tididi_bonsai_count_clauses over TWO_MODELS_LITERALS, with no limits. */
static tididi_bonsai_code count_two_models(const uint32_t *show, size_t show_len,
                                           const tididi_bonsai_weight *weights, size_t weights_len,
                                           const char *mode, tididi_bonsai_answer **out) {
  return tididi_bonsai_count_clauses(3, TWO_MODELS_LITERALS, TWO_MODELS_LEN, show, show_len, weights,
                                     weights_len, mode, 0, 0, out);
}

/* tididi_bonsai_count over NUL-terminated text, with no limits. */
static tididi_bonsai_code count_text(const char *dimacs, const char *mode,
                                     tididi_bonsai_answer **out) {
  return tididi_bonsai_count((const uint8_t *)dimacs, strlen(dimacs), mode, 0, 0, out);
}

static const char *message_of(const tididi_bonsai_answer *answer) {
  return or_null(tididi_bonsai_answer_error_message(answer, NULL));
}

/* The answer must be a count with this mode, verdict and count. */
static void expect_count(const char *what, tididi_bonsai_code code, const tididi_bonsai_answer *answer,
                         const char *mode, int satisfiable, const char *count) {
  size_t len = 99;
  const char *text;

  CHECK(code == TIDIDI_BONSAI_OK, "%s: code %d: %s", what, code, message_of(answer));
  if (code != TIDIDI_BONSAI_OK) return;
  CHECK(tididi_bonsai_answer_code(answer) == code, "%s: the answer's code differs", what);
  text = tididi_bonsai_answer_mode(answer, &len);
  CHECK(text && strcmp(text, mode) == 0 && len == strlen(mode), "%s: mode %s, expected %s", what,
        or_null(text), mode);
  CHECK(tididi_bonsai_answer_satisfiable(answer) == (satisfiable != 0), "%s: satisfiable is not %d",
        what, satisfiable);
  text = tididi_bonsai_answer_count(answer, &len);
  CHECK(text && strcmp(text, count) == 0 && len == strlen(count), "%s: count %s, expected %s", what,
        or_null(text), count);
  CHECK(tididi_bonsai_answer_error_message(answer, &len) == NULL && len == 0,
        "%s: a count has no error message", what);
}

/* The answer must be a failure with this code, whose message mentions
 * `needle`, and lend nothing a count would. */
static void expect_error(const char *what, tididi_bonsai_code code, const tididi_bonsai_answer *answer,
                         tididi_bonsai_code expected, const char *needle) {
  size_t len = 99;

  CHECK(code == expected, "%s: code %d, expected %d: %s", what, code, expected, message_of(answer));
  CHECK(tididi_bonsai_answer_code(answer) == code, "%s: the answer's code differs", what);
  CHECK(strstr(message_of(answer), needle) != NULL, "%s: the message does not mention \"%s\": %s",
        what, needle, message_of(answer));
  CHECK(tididi_bonsai_answer_mode(answer, &len) == NULL && len == 0, "%s: a failure lends a mode", what);
  CHECK(tididi_bonsai_answer_count(answer, &len) == NULL && len == 0, "%s: a failure lends a count", what);
  CHECK(tididi_bonsai_answer_stats_json(answer, &len) == NULL && len == 0,
        "%s: a failure lends a record", what);
  CHECK(!tididi_bonsai_answer_satisfiable(answer), "%s: a failure is satisfiable", what);
  CHECK(isnan(tididi_bonsai_answer_count_f64(answer)), "%s: a failure's count is not NaN", what);
}

/* The whole file at `path` in a NUL-terminated buffer the caller frees, or
 * NULL. */
static char *read_file(const char *path) {
  FILE *file = fopen(path, "rb");
  char *data = NULL;
  long size;

  if (file == NULL) return NULL;
  if (fseek(file, 0, SEEK_END) == 0 && (size = ftell(file)) >= 0 && fseek(file, 0, SEEK_SET) == 0 &&
      (data = malloc((size_t)size + 1)) != NULL) {
    data[fread(data, 1, (size_t)size, file)] = '\0';
  }
  fclose(file);
  return data;
}

/* Read DIMACS text into `out`, with the line kinds the instances in
 * bindings/parity.txt use: `p cnf`, `c p show`, `c p weight`, other comments
 * and clause lines. Returns 0 when it all fit. */
static int read_clauses(const char *text, clauses *out) {
  out->num_vars = 0;
  out->literals_len = out->show_len = out->weights_len = 0;
  out->has_show = 0;
  while (*text != '\0') {
    char line[LINE_SIZE];
    size_t len = strcspn(text, "\n");
    char *at, *next;
    long value;

    if (len >= sizeof line) return -1;
    memcpy(line, text, len);
    line[len] = '\0';
    text += len + (text[len] == '\n');
    if (strncmp(line, "c p show ", 9) == 0) {
      out->has_show = 1;
      for (at = line + 9; (value = strtol(at, &next, 10)) != 0; at = next) {
        if (out->show_len == MAX_NAMED) return -1;
        out->show[out->show_len++] = (uint32_t)value;
      }
    } else if (strncmp(line, "c p weight ", 11) == 0) {
      size_t n = out->weights_len;
      if (n == MAX_NAMED) return -1;
      out->weights[n].literal = (int32_t)strtol(line + 11, &next, 10);
      if (sscanf(next, "%63s", out->weight_text[n]) != 1) return -1;
      out->weights[n].weight = out->weight_text[n];
      out->weights_len++;
    } else if (strncmp(line, "p cnf ", 6) == 0) {
      out->num_vars = (uint32_t)strtoul(line + 6, NULL, 10);
    } else if (line[0] != 'c' && line[0] != '%') {
      for (at = line; value = strtol(at, &next, 10), next != at; at = next) {
        if (out->literals_len == MAX_LITERALS) return -1;
        out->literals[out->literals_len++] = (int32_t)value;
      }
    }
  }
  return 0;
}

/* Run the tididi executable on `path` in `mode` with the competition's output
 * format, and read its verdict and the count, which is the last word of its
 * `c s exact` line. Returns 0 when both were found. */
static int run_tididi(const char *executable, const char *path, const char *mode, int *satisfiable,
                      char *count) {
  char command[3 * LINE_SIZE];
  char line[LINE_SIZE];
  int found = 0;
  FILE *output;

  if (snprintf(command, sizeof command,
               "'%s' --mode %s --output-format mcc --mem-limit-mb 0 '%s' 2>/dev/null", executable,
               mode, path) >= (int)sizeof command)
    return -1;
  output = popen(command, "r");
  if (output == NULL) return -1;
  while (fgets(line, sizeof line, output) != NULL) {
    char *last;
    line[strcspn(line, "\n")] = '\0';
    if (strcmp(line, "s SATISFIABLE") == 0 || strcmp(line, "s UNSATISFIABLE") == 0) {
      *satisfiable = line[2] == 'S';
      found |= 1;
    } else if (strncmp(line, "c s exact ", 10) == 0 && (last = strrchr(line, ' ')) != NULL) {
      strcpy(count, last + 1);
      found |= 2;
    }
  }
  return pclose(output) == 0 && found == 3 ? 0 : -1;
}

/* The library reports the version of tididi-bonsai it was built from, which
 * the executable built from the same source prints for --version. */
static void test_versions(const char *executable) {
  char command[2 * LINE_SIZE];
  char expected[LINE_SIZE];
  char printed[LINE_SIZE] = "";
  FILE *output;

  CHECK(tididi_bonsai_abi_version() == TIDIDI_BONSAI_ABI_VERSION, "ABI version %u, header %u",
        tididi_bonsai_abi_version(), TIDIDI_BONSAI_ABI_VERSION);
  snprintf(expected, sizeof expected, "tididi %s\n", tididi_bonsai_version());
  if (snprintf(command, sizeof command, "'%s' --version", executable) >= (int)sizeof command)
    output = NULL;
  else
    output = popen(command, "r");
  CHECK(output != NULL, "cannot run %s --version", executable);
  if (output == NULL) return;
  if (fgets(printed, sizeof printed, output) == NULL) printed[0] = '\0';
  CHECK(pclose(output) == 0, "%s --version failed", executable);
  CHECK(strcmp(printed, expected) == 0, "the library is version %s, the executable prints %s",
        tididi_bonsai_version(), printed);
}

/* Each listed instance, read from its file, from its text and as clauses,
 * counts as the executable counts it. */
static void test_counts_match_the_executable(const char *executable, const char *root) {
  static clauses formula;
  char list_path[LINE_SIZE];
  char line[LINE_SIZE];
  int instances = 0;
  FILE *list;

  if (snprintf(list_path, sizeof list_path, "%s/bindings/parity.txt", root) >= (int)sizeof list_path)
    list = NULL;
  else
    list = fopen(list_path, "r");
  CHECK(list != NULL, "cannot open %s", list_path);
  if (list == NULL) return;
  while (fgets(line, sizeof line, list) != NULL) {
    char relative[LINE_SIZE], mode[LINE_SIZE], path[2 * LINE_SIZE], count[LINE_SIZE];
    int satisfiable = -1;
    tididi_bonsai_answer *answer = NULL;
    tididi_bonsai_code code;
    char *text;

    if (line[0] == '#' || sscanf(line, "%4095s %4095s", relative, mode) != 2) continue;
    instances++;
    if (snprintf(path, sizeof path, "%s/%s", root, relative) >= (int)sizeof path) {
      CHECK(0, "%s/%s is too long a path", root, relative);
      continue;
    }
    if (run_tididi(executable, path, mode, &satisfiable, count) != 0) {
      CHECK(0, "%s: the executable gave no answer in mode %s", relative, mode);
      continue;
    }

    code = tididi_bonsai_count_file(path, mode, 0, 0, &answer);
    expect_count(relative, code, answer, mode, satisfiable, count);
    tididi_bonsai_answer_free(answer);

    text = read_file(path);
    CHECK(text != NULL, "cannot read %s", path);
    if (text == NULL) continue;
    code = count_text(text, mode, &answer);
    expect_count(relative, code, answer, mode, satisfiable, count);
    tididi_bonsai_answer_free(answer);

    if (read_clauses(text, &formula) != 0) {
      CHECK(0, "%s does not fit the clause buffers", relative);
    } else {
      code = tididi_bonsai_count_clauses(formula.num_vars, formula.literals, formula.literals_len,
                                         formula.has_show ? formula.show : NULL, formula.show_len,
                                         formula.weights, formula.weights_len, mode, 0, 0, &answer);
      expect_count(relative, code, answer, mode, satisfiable, count);
      tididi_bonsai_answer_free(answer);
    }
    free(text);
  }
  fclose(list);
  CHECK(instances > 0, "%s lists no instance", list_path);
}

static void test_counts(void) {
  tididi_bonsai_answer *answer = NULL;
  tididi_bonsai_code code;
  const char *stats;

  code = count_text(TWO_MODELS, "mc", &answer);
  expect_count("mc", code, answer, "mc", 1, "2");
  stats = tididi_bonsai_answer_stats_json(answer, NULL);
  CHECK(stats && strstr(stats, "\"mode\":\"mc\"") && strstr(stats, "\"model_count\":\"2\""),
        "the record does not carry the mode and the count: %s", or_null(stats));
  tididi_bonsai_answer_free(answer);

  /* Without a mode, the header chooses, and a formula without one is
   * compiled. */
  code = count_text(TWO_MODELS_MC, NULL, &answer);
  expect_count("the header's mode", code, answer, "mc", 1, "2");
  tididi_bonsai_answer_free(answer);
  code = count_text(TWO_MODELS, NULL, &answer);
  expect_count("no mode", code, answer, "compile", 1, "2");
  tididi_bonsai_answer_free(answer);

  code = count_text(REFUTED, "mc", &answer);
  expect_count("refuted", code, answer, "mc", 0, "0");
  tididi_bonsai_answer_free(answer);

  /* The weighted count is in the log domain, so its last digits round. */
  code = count_text(WEIGHTED, NULL, &answer);
  CHECK(code == TIDIDI_BONSAI_OK && near(tididi_bonsai_answer_count_f64(answer), 0.65),
        "the weighted count is %s", or_null(tididi_bonsai_answer_count(answer, NULL)));
  tididi_bonsai_answer_free(answer);
}

static void test_clauses(void) {
  /* TWO_MODELS counted over x1 and x2, with x1 weighing 0.3 true and 0.7
   * false and x2 2/5 true, so 3/5 false: 0.3 * 3/5 + 0.7 * 2/5 = 23/50. */
  static const uint32_t shown[] = {1, 2};
  const tididi_bonsai_weight weights[] = {{1, "0.3"}, {-1, "0.7"}, {2, "2/5"}};
  tididi_bonsai_answer *answer = NULL;
  tididi_bonsai_code code;

  code = count_two_models(NULL, 0, NULL, 0, "mc", &answer);
  expect_count("clauses", code, answer, "mc", 1, "2");
  CHECK(tididi_bonsai_answer_count_f64(answer) == 2.0, "the count as a double is %g",
        tididi_bonsai_answer_count_f64(answer));
  tididi_bonsai_answer_free(answer);

  code = count_two_models(shown, 2, weights, 3, "pwmc", &answer);
  expect_count("weighted clauses", code, answer, "pwmc", 1, "23/50");
  CHECK(near(tididi_bonsai_answer_count_f64(answer), 0.46), "the count as a double is %g",
        tididi_bonsai_answer_count_f64(answer));
  tididi_bonsai_answer_free(answer);

  /* A null show declares no show set, so the count is over every variable;
   * an empty one projects onto no variable, leaving only the verdict. */
  code = count_two_models(NULL, 0, NULL, 0, "pmc", &answer);
  expect_count("no show set", code, answer, "pmc", 1, "2");
  tididi_bonsai_answer_free(answer);
  code = count_two_models(shown, 0, NULL, 0, "pmc", &answer);
  expect_count("an empty show set", code, answer, "pmc", 1, "1");
  tididi_bonsai_answer_free(answer);

  /* Without a mode, a show set makes the count projected and anything else
   * compiles. */
  code = count_two_models(NULL, 0, NULL, 0, NULL, &answer);
  expect_count("clauses with no mode", code, answer, "compile", 1, "2");
  tididi_bonsai_answer_free(answer);
  code = count_two_models(shown, 1, NULL, 0, NULL, &answer);
  expect_count("clauses with a show set", code, answer, "pmc", 1, "2");
  tididi_bonsai_answer_free(answer);

  /* No clauses: every assignment of the one variable is a model. */
  code = tididi_bonsai_count_clauses(1, NULL, 0, NULL, 0, NULL, 0, "mc", 0, 0, &answer);
  expect_count("no clauses", code, answer, "mc", 1, "2");
  tididi_bonsai_answer_free(answer);
}

static void test_errors(const char *root) {
  tididi_bonsai_answer *answer = NULL;
  tididi_bonsai_code code;
  char path[LINE_SIZE];

  code = count_text(MALFORMED, "mc", &answer);
  expect_error("malformed", code, answer, TIDIDI_BONSAI_ERROR_INPUT, "x");
  tididi_bonsai_answer_free(answer);

  code = count_text(TWO_MODELS, "weighted", &answer);
  expect_error("unknown mode", code, answer, TIDIDI_BONSAI_ERROR_USAGE, "weighted");
  tididi_bonsai_answer_free(answer);

  code = count_text(WEIGHTED, "mc", &answer);
  expect_error("a mode the header contradicts", code, answer, TIDIDI_BONSAI_ERROR_USAGE, "mc");
  tididi_bonsai_answer_free(answer);

  {
    static const int32_t past[] = {1, 4, 0};
    static const int32_t twice[] = {1, -1, 0};
    static const int32_t no_variable[] = {INT32_MIN, 0};
    static const uint32_t zero[] = {0};
    const tididi_bonsai_weight heavy[] = {{1, "heavy"}};
    const tididi_bonsai_weight half[] = {{1, "1/2"}};

    code = tididi_bonsai_count_clauses(3, past, 3, NULL, 0, NULL, 0, "mc", 0, 0, &answer);
    expect_error("a variable past num_vars", code, answer, TIDIDI_BONSAI_ERROR_INPUT, "variable 4");
    tididi_bonsai_answer_free(answer);
    code = tididi_bonsai_count_clauses(3, twice, 3, NULL, 0, NULL, 0, "mc", 0, 0, &answer);
    expect_error("a clause naming a variable twice", code, answer, TIDIDI_BONSAI_ERROR_INPUT, "twice");
    tididi_bonsai_answer_free(answer);
    code = tididi_bonsai_count_clauses(3, no_variable, 2, NULL, 0, NULL, 0, "mc", 0, 0, &answer);
    expect_error("INT32_MIN", code, answer, TIDIDI_BONSAI_ERROR_INPUT, "names no variable");
    tididi_bonsai_answer_free(answer);
    code = count_two_models(zero, 1, NULL, 0, "pmc", &answer);
    expect_error("show variable 0", code, answer, TIDIDI_BONSAI_ERROR_INPUT, "0 is not a variable");
    tididi_bonsai_answer_free(answer);
    code = count_two_models(NULL, 0, heavy, 1, "wmc", &answer);
    expect_error("a weight that is no number", code, answer, TIDIDI_BONSAI_ERROR_INPUT,
                 "the weight of literal 1");
    tididi_bonsai_answer_free(answer);
    code = count_two_models(NULL, 0, half, 1, "mc", &answer);
    expect_error("weights in an unweighted count", code, answer, TIDIDI_BONSAI_ERROR_USAGE, "weight");
    tididi_bonsai_answer_free(answer);
  }

  if (snprintf(path, sizeof path, "%s/bindings/no-such-file.cnf", root) >= (int)sizeof path) {
    CHECK(0, "%s is too long a path", root);
    return;
  }
  code = tididi_bonsai_count_file(path, "mc", 0, 0, &answer);
  expect_error("missing file", code, answer, TIDIDI_BONSAI_ERROR_INPUT, "no-such-file.cnf");
  tididi_bonsai_answer_free(answer);
}

static void test_argument_contract(void) {
  tididi_bonsai_answer *answer = NULL;
  tididi_bonsai_code code;
  size_t len = 99;

  CHECK(count_text(TWO_MODELS, "mc", NULL) == TIDIDI_BONSAI_ERROR_INVALID_ARGUMENT,
        "a null out is refused");
  CHECK(tididi_bonsai_count_file("x.cnf", "mc", 0, 0, NULL) == TIDIDI_BONSAI_ERROR_INVALID_ARGUMENT,
        "a null out is refused");

  code = tididi_bonsai_count(NULL, 3, "mc", 0, 0, &answer);
  expect_error("null dimacs", code, answer, TIDIDI_BONSAI_ERROR_INVALID_ARGUMENT, "dimacs_len is 3");
  tididi_bonsai_answer_free(answer);

  code = tididi_bonsai_count_file(NULL, "mc", 0, 0, &answer);
  expect_error("null path", code, answer, TIDIDI_BONSAI_ERROR_INVALID_ARGUMENT, "path is null");
  tididi_bonsai_answer_free(answer);

  /* The last literal closes a clause, which catches a short literals_len. */
  code = tididi_bonsai_count_clauses(3, TWO_MODELS_LITERALS, TWO_MODELS_LEN - 1, NULL, 0, NULL, 0, "mc",
                                     0, 0, &answer);
  expect_error("an unclosed clause", code, answer, TIDIDI_BONSAI_ERROR_INVALID_ARGUMENT,
               "literals ends with -3");
  tididi_bonsai_answer_free(answer);
  code = tididi_bonsai_count_clauses(3, NULL, 3, NULL, 0, NULL, 0, "mc", 0, 0, &answer);
  expect_error("null literals", code, answer, TIDIDI_BONSAI_ERROR_INVALID_ARGUMENT, "literals_len is 3");
  tididi_bonsai_answer_free(answer);
  code = count_two_models(NULL, 2, NULL, 0, "pmc", &answer);
  expect_error("null show", code, answer, TIDIDI_BONSAI_ERROR_INVALID_ARGUMENT, "show_len is 2");
  tididi_bonsai_answer_free(answer);
  {
    const tididi_bonsai_weight unwritten[] = {{1, NULL}};
    code = count_two_models(NULL, 0, unwritten, 1, "wmc", &answer);
    expect_error("a null weight", code, answer, TIDIDI_BONSAI_ERROR_INVALID_ARGUMENT,
                 "weights[0].weight is null");
    tididi_bonsai_answer_free(answer);
  }
  CHECK(isnan(tididi_bonsai_answer_count_f64(NULL)), "a null answer's count is not NaN");

  /* Only the first dimacs_len bytes are read: the rest is not DIMACS. */
  {
    char text[sizeof TWO_MODELS + 8];
    memcpy(text, TWO_MODELS, strlen(TWO_MODELS));
    memcpy(text + strlen(TWO_MODELS), "garbage", 8);
    code = tididi_bonsai_count((const uint8_t *)text, strlen(TWO_MODELS), "mc", 0, 0, &answer);
    expect_count("length-bounded text", code, answer, "mc", 1, "2");
    tididi_bonsai_answer_free(answer);
  }

  CHECK(tididi_bonsai_answer_code(NULL) == TIDIDI_BONSAI_ERROR_INVALID_ARGUMENT, "a null answer's code");
  CHECK(tididi_bonsai_answer_count(NULL, &len) == NULL && len == 0, "a null answer lends nothing");
  CHECK(tididi_bonsai_answer_error_message(NULL, &len) == NULL && len == 0, "a null answer lends nothing");
  CHECK(!tididi_bonsai_answer_satisfiable(NULL), "a null answer is not satisfiable");
  tididi_bonsai_answer_free(NULL);
}

static void test_repeated_calls(void) {
  for (int i = 0; i < 3; i++) {
    tididi_bonsai_answer *answer = NULL;
    tididi_bonsai_code code = count_text(TWO_MODELS, "mc", &answer);
    expect_count("repeated", code, answer, "mc", 1, "2");
    tididi_bonsai_answer_free(answer);
  }
}

/* One call made on a thread of its own, and what it returned. */
struct call {
  pthread_t thread;
  tididi_bonsai_code code;
  tididi_bonsai_answer *answer;
};

static void *count_on_a_thread(void *arg) {
  struct call *call = arg;
  call->code = count_text(TWO_MODELS, "mc", &call->answer);
  return NULL;
}

/* Each thread's call gets its answer. The answers are checked here, after
 * each thread is joined, since the checks share one failure count. */
static void test_concurrent_calls(void) {
  struct call calls[4];
  for (int i = 0; i < 4; i++) {
    calls[i].answer = NULL;
    CHECK(pthread_create(&calls[i].thread, NULL, count_on_a_thread, &calls[i]) == 0,
          "cannot start thread %d", i);
  }
  for (int i = 0; i < 4; i++) {
    pthread_join(calls[i].thread, NULL);
    expect_count("concurrent", calls[i].code, calls[i].answer, "mc", 1, "2");
    tididi_bonsai_answer_free(calls[i].answer);
  }
}

int main(int argc, char **argv) {
  if (argc != 3) {
    fprintf(stderr, "usage: %s TIDIDI_EXECUTABLE REPOSITORY_ROOT\n", argv[0]);
    return 2;
  }
  test_versions(argv[1]);
  test_counts_match_the_executable(argv[1], argv[2]);
  test_counts();
  test_clauses();
  test_errors(argv[2]);
  test_argument_contract();
  test_repeated_calls();
  test_concurrent_calls();
  if (failures) {
    printf("%d check(s) failed\n", failures);
    return 1;
  }
  printf("all checks passed\n");
  return 0;
}
