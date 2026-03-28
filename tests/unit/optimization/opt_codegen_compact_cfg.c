/* opt_codegen_compact_cfg.c - codegen CFG compaction coverage */

int status;

void main(void)
{
    int x;
    int y;

    x = 0;
    y = 0;

    /* positive: jump chain */
    if (x == 0) {
        goto A;
    }
    goto B;
A:
    goto C;
B:
    y = y + 10;
C:
    y = y + 1;

    /* negative/safety: branch with useful body */
    if (y > 0) {
        y = y + 2;
    } else {
        y = y + 3;
    }

    status = y;
}
