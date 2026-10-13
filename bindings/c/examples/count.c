/* Count the models of a DIMACS CNF file through the C API, and print the mode
 * the run resolved to, whether the formula has a model, and the count.
 *
 *   count CNF [MODE]
 *
 * MODE is compile, mc, wmc, pmc or pwmc; without it the instance's header
 * chooses. The run has a minute and leaves the address-space limit as it is.
 * For tests/data/queens5.cnf, five queens on a 5x5 board, it prints:
 *
 *   mode: compile
 *   satisfiable: yes
 *   count: 10
 */

#include <stdio.h>

#include "tididi_bonsai.h"

int main(int argc, char **argv) {
  tididi_bonsai_answer *answer = NULL;
  tididi_bonsai_code code;

  if (argc != 2 && argc != 3) {
    fprintf(stderr, "usage: %s CNF [MODE]\n", argv[0]);
    return 2;
  }
  if (tididi_bonsai_abi_version() != TIDIDI_BONSAI_ABI_VERSION) {
    fprintf(stderr, "the loaded library has ABI %u, this program was built for %u\n",
            tididi_bonsai_abi_version(), TIDIDI_BONSAI_ABI_VERSION);
    return 1;
  }

  code = tididi_bonsai_count_file(argv[1], argc == 3 ? argv[2] : NULL, 60000, 0, &answer);
  if (code != TIDIDI_BONSAI_OK) {
    fprintf(stderr, "%s (code %d)\n", tididi_bonsai_answer_error_message(answer, NULL), code);
    tididi_bonsai_answer_free(answer);
    return 1;
  }
  printf("mode: %s\n", tididi_bonsai_answer_mode(answer, NULL));
  printf("satisfiable: %s\n", tididi_bonsai_answer_satisfiable(answer) ? "yes" : "no");
  printf("count: %s\n", tididi_bonsai_answer_count(answer, NULL));
  tididi_bonsai_answer_free(answer);
  return 0;
}
