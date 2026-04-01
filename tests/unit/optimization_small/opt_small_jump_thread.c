// opt_small_jump_thread.c - jump threading
//
// Feature: redirect jumps to jump targets, eliminating intermediate branches.
// Benefit: reduce branches and flatten control flow.

int cond;
int out;

int main(void) {
    cond = 1;
    if (cond) {
        out = 100;  /* taken */
    } else {
        out = 200;
    }
    if (out != 100) return 1;
    return 0;
}
