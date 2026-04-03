Create a plan for building a C compiler targeting the Intel 8080. Before defining the plan, analyze and audit several existing C‑to‑8080 compilers with respect to performance and supported functionality:

https://github.com/alemorf/c8080/blob/main/doc/function_args.txt https://github.com/z88dk/z88dk/wiki/Benchmarks

The goal is to produce the most performant 8080 machine code possible. Support for C language features will be minimal initially, but the design should allow for straightforward future expansion. Store the resulting plan in the root of the workspace as plan.md.