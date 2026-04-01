/* opt_ir_loop.c - IR loop pass coverage */

int main(void)
{
    int i;
    int pos;
    int neg;
    int safe;

    /* positive: sum 0..15 = 120 */
    pos = 0;
    for (i = 0; i < 16; i = i + 1) {
        pos = pos + i;
    }

    /* negative: 5+4+3+2+1 = 15 */
    i = 5;
    neg = 0;
    while (i > 0) {
        neg = neg + i;
        i = i - 1;
    }

    /* safety: safe starts 1, adds 0+1+2 = 4 */
    i = 0;
    safe = 1;
    do {
        safe = safe + i;
        i = i + 1;
    } while (i < 3);

    if (pos != 120) return 1;
    if (neg != 15) return 2;
    if (safe != 4) return 3;
    return 0;
}
