/* opt_ir_jump_thread.c - IR jump threading coverage */

int main(void)
{
    int x;
    int pos;
    int neg;
    int safe;

    x = 0;
    pos = 0;
    neg = 0;
    safe = 0;

    /* positive: x==0 → L1 → L3; pos=0+1=1 */
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

    /* negative: x!=0 false → else: neg=20 */
    if (x != 0) {
        neg = neg + 10;
    } else {
        neg = neg + 20;
    }

    /* safety: neg>0 → safe=7 */
    if (neg > 0) {
        safe = 7;
    } else {
        safe = 9;
    }

    if (pos != 1) return 1;
    if (neg != 20) return 2;
    if (safe != 7) return 3;
    return 0;
}
