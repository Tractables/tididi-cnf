// Count the models of a DIMACS CNF file through the C API, and print the mode
// the run resolved to, whether the formula has a model, and the count: the C
// example count.c in C++17, with the answer held by a std::unique_ptr.
//
//   count CNF [MODE]
//
// For tests/data/queens5.cnf it prints what count.c prints.

#include <iostream>
#include <memory>

#include "tididi_cnf.h"

namespace {

struct AnswerFree {
  void operator()(tididi_cnf_answer *answer) const { tididi_cnf_answer_free(answer); }
};

using Answer = std::unique_ptr<tididi_cnf_answer, AnswerFree>;

}  // namespace

int main(int argc, char **argv) {
  if (argc != 2 && argc != 3) {
    std::cerr << "usage: " << argv[0] << " CNF [MODE]\n";
    return 2;
  }
  if (tididi_cnf_abi_version() != TIDIDI_CNF_ABI_VERSION) {
    std::cerr << "the loaded library has ABI " << tididi_cnf_abi_version()
              << ", this program was built for " << TIDIDI_CNF_ABI_VERSION << '\n';
    return 1;
  }

  tididi_cnf_answer *raw = nullptr;
  const tididi_cnf_code code =
      tididi_cnf_count_file(argv[1], argc == 3 ? argv[2] : nullptr, 60000, 0, &raw);
  const Answer answer{raw};
  if (code != TIDIDI_CNF_OK) {
    std::cerr << tididi_cnf_answer_error_message(answer.get(), nullptr) << " (code " << code
              << ")\n";
    return 1;
  }
  std::cout << "mode: " << tididi_cnf_answer_mode(answer.get(), nullptr) << '\n'
            << "satisfiable: " << (tididi_cnf_answer_satisfiable(answer.get()) ? "yes" : "no")
            << '\n'
            << "count: " << tididi_cnf_answer_count(answer.get(), nullptr) << '\n';
  return 0;
}
