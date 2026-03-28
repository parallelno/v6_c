/* opt_ir_jump_thread.c - IR jump threading coverage */

int status;

void main(void)
{
    int x;
    int pos;
    int neg;
    int safe;

    x = 0;
    pos = 0;
    neg = 0;
    safe = 0;

    /* positive: chainable jump flow */
    if (x == 0) {
        goto L1;
    }
    goto L2;
L1:
    goto L3;
L2:
    pos = 100;
L3:
    pos = pos + 1;

    /* negative: non-threadable edge with side-effecting work */
    if (x != 0) {
        neg = neg + 10;
    } else {
        neg = neg + 20;
    }

    /* safety: additional branch fan-out */
    if (neg > 0) {
        safe = 7;
    } else {
        safe = 9;
    }

    status = pos + neg + safe;
}
