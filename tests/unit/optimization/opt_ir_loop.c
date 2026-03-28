/* opt_ir_loop.c - IR loop pass coverage */

int status;

void main(void)
{
    int i;
    int pos;
    int neg;
    int safe;

    /* positive: for-loop accumulation */
    pos = 0;
    for (i = 0; i < 16; i = i + 1) {
        pos = pos + i;
    }

    /* negative: while-loop with data dependency */
    i = 5;
    neg = 0;
    while (i > 0) {
        neg = neg + i;
        i = i - 1;
    }

    /* safety: do-while executes at least once */
    i = 0;
    safe = 1;
    do {
        safe = safe + i;
        i = i + 1;
    } while (i < 3);

    status = pos + neg + safe;
}
