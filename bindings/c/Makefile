# Builds the C bindings and the programs that use them, on Linux. Run from
# bindings/c; README.md describes each target.
#
#   make                 the shared and static libraries, and the tididi executable
#   make check           the examples and the conformance tests against each library
#   make link-libs-check Libs.private in tididi-cnf-c.pc.in covers what rustc reports
#   make header          regenerate include/tididi_cnf.h
#   make header-check    fail if include/tididi_cnf.h differs from cbindgen's output

SHELL := bash
.SHELLFLAGS := -eo pipefail -c

CARGO ?= cargo
CBINDGEN ?= cbindgen
CC ?= cc
CXX ?= c++
CFLAGS ?= -O2 -g
CXXFLAGS ?= -O2 -g

ROOT := ../..
RELEASE := target/release
OUT := target/c
SHARED := $(RELEASE)/libtididi_cnf_c.so
STATIC := $(RELEASE)/libtididi_cnf_c.a
# The binary the conformance tests compare counts with, built from the same
# checkout by the workspace above.
TIDIDI := $(ROOT)/target/release/tididi

# The system libraries a static link needs. tididi-cnf-c.pc.in is the one
# place they are listed.
STATIC_LIBS := $(shell sed -n 's/^Libs.private: //p' tididi-cnf-c.pc.in)
LINK_shared := -L$(RELEASE) -ltididi_cnf_c -Wl,-rpath,$(abspath $(RELEASE))
LINK_static := $(STATIC) $(STATIC_LIBS)

WARNINGS := -Wall -Wextra -Werror
KINDS := shared static
EXAMPLES := count count-cpp clauses
PROGRAMS := $(foreach kind,$(KINDS),$(foreach example,$(EXAMPLES),$(OUT)/$(example)-$(kind)) $(OUT)/conformance-$(kind))

# What examples/count.c and examples/count.cpp promise for this instance in
# their header comments.
EXAMPLE_CNF := $(ROOT)/tests/data/queens5.cnf
EXAMPLE_OUTPUT := mode: compile\nsatisfiable: yes\ncount: 10
# What examples/clauses.c promises in its header comment.
CLAUSES_OUTPUT := mode: pwmc\nsatisfiable: yes\ncount: 23/50\nas a double: 0.46

.PHONY: all libs check link-libs-check header header-check clean

all: libs

# rustc prints the static link line only when it compiles this crate, so the
# crate is touched to make every run print it. `--locked` builds the
# dependency versions the committed Cargo.lock names.
libs:
	@mkdir -p $(OUT)
	touch src/lib.rs
	$(CARGO) rustc --release --locked --lib -- --print native-static-libs 2>&1 | tee $(OUT)/rustc.log
	cd $(ROOT) && $(CARGO) build --release --locked --bin tididi

# cargo decides what is stale, so the libraries and the binary are rebuilt on
# every run; a program relinks when its library came out of that newer.
$(SHARED) $(STATIC) $(TIDIDI): libs

$(OUT)/count-%: examples/count.c include/tididi_cnf.h $(SHARED) $(STATIC)
	$(CC) -std=c99 $(WARNINGS) $(CFLAGS) -Iinclude $< $(LINK_$*) -o $@

$(OUT)/count-cpp-%: examples/count.cpp include/tididi_cnf.h $(SHARED) $(STATIC)
	$(CXX) -std=c++17 $(WARNINGS) $(CXXFLAGS) -Iinclude $< $(LINK_$*) -o $@

$(OUT)/clauses-%: examples/clauses.c include/tididi_cnf.h $(SHARED) $(STATIC)
	$(CC) -std=c99 $(WARNINGS) $(CFLAGS) -Iinclude $< $(LINK_$*) -o $@

$(OUT)/conformance-%: tests/conformance.c include/tididi_cnf.h $(SHARED) $(STATIC)
	$(CC) -std=c99 $(WARNINGS) $(CFLAGS) -pthread -Iinclude $< $(LINK_$*) -o $@

# The runs' progress lines go to stderr, kept in $(OUT) beside the programs;
# the examples' stdout and the test driver's failures are on stdout.
check: $(PROGRAMS)
	for kind in $(KINDS); do \
	  for count in count count-cpp; do \
	    diff <(printf '$(EXAMPLE_OUTPUT)\n') \
	      <($(OUT)/$$count-$$kind $(EXAMPLE_CNF) 2> $(OUT)/$$count-$$kind.stderr); \
	  done; \
	  diff <(printf '$(CLAUSES_OUTPUT)\n') <($(OUT)/clauses-$$kind 2> $(OUT)/clauses-$$kind.stderr); \
	  $(OUT)/conformance-$$kind $(TIDIDI) $(abspath $(ROOT)) 2> $(OUT)/conformance-$$kind.stderr; \
	done

link-libs-check:
	libs=$$(sed -n 's/^note: native-static-libs: //p' $(OUT)/rustc.log | head -n 1); \
	test -n "$$libs" || { echo "no native-static-libs note in $(OUT)/rustc.log; run make libs"; exit 1; }; \
	echo "rustc: $$libs"; \
	missing=; \
	for lib in $$libs; do \
	  case " $(STATIC_LIBS) " in *" $$lib "*) ;; *) missing="$$missing $$lib" ;; esac; \
	done; \
	test -z "$$missing" || { echo "Libs.private in tididi-cnf-c.pc.in lacks:$$missing"; exit 1; }

header:
	$(CBINDGEN) --config cbindgen.toml --crate tididi-cnf-c --output include/tididi_cnf.h .

header-check:
	@mkdir -p $(OUT)
	$(CBINDGEN) --config cbindgen.toml --crate tididi-cnf-c --output $(OUT)/tididi_cnf.h .
	diff -u include/tididi_cnf.h $(OUT)/tididi_cnf.h || { \
	  echo "include/tididi_cnf.h is out of date; regenerate it with make header"; exit 1; }

clean:
	rm -rf $(OUT)
