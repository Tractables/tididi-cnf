/* Count a formula the program builds, handed to the C API as clauses of
 * DIMACS integers, and print the mode, whether the formula has a model, the
 * exact count and the count as a double.
 *
 *   clauses
 *
 * The formula is (x1 or x2) and (not x1 or x3) and (not x2 or not x3),
 * counted over x1 and x2, with x1 weighing 0.3 true and 0.7 false, and x2
 * 0.5 true. It prints:
 *
 *   mode: pwmc
 *   satisfiable: yes
 *   count: 13/20
 *   as a double: 0.65
 */

#include <stdio.h>

#include "tididi_bonsai.h"

int main(void) {
  /* Each clause is closed by 0, as in a DIMACS file. */
  static const int32_t literals[] = {1, 2, 0, -1, 3, 0, -2, -3, 0};
  static const uint32_t show[] = {1, 2};
  /* A weight is text, so it stays exact. */
  const tididi_bonsai_weight weights[] = {{1, "0.3"}, {-1, "0.7"}, {2, "0.5"}};
  tididi_bonsai_answer *answer = NULL;
  tididi_bonsai_code code;

  code = tididi_bonsai_count_clauses(3, literals, sizeof literals / sizeof *literals, show,
                                     sizeof show / sizeof *show, weights, sizeof weights / sizeof *weights,
                                     "pwmc", 60000, 0, &answer);
  if (code != TIDIDI_BONSAI_OK) {
    fprintf(stderr, "%s (code %d)\n", tididi_bonsai_answer_error_message(answer, NULL), code);
    tididi_bonsai_answer_free(answer);
    return 1;
  }
  printf("mode: %s\n", tididi_bonsai_answer_mode(answer, NULL));
  printf("satisfiable: %s\n", tididi_bonsai_answer_satisfiable(answer) ? "yes" : "no");
  printf("count: %s\n", tididi_bonsai_answer_count(answer, NULL));
  printf("as a double: %g\n", tididi_bonsai_answer_count_f64(answer));
  tididi_bonsai_answer_free(answer);
  return 0;
}
