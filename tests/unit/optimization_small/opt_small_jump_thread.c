// opt_small_jump_thread.c - jump threading
//
// Feature: redirect jumps to jump targets, eliminating intermediate branches.
// Benefit: reduce branches and flatten control flow.
// Example:
//   if (cond) goto L1; else goto L2; L1: goto L3; -> if(cond) goto L3;...

int cond;
int out;

void main(void) {
    cond = 1;
    if (cond) {
        out = 100;
    } else {
        out = 200;
    }
}
