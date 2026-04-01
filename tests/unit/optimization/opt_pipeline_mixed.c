/* opt_pipeline_mixed.c - mixed pass interaction coverage */

int f(int x)
{
    return (x * 2) + 1;
}

int main(void)
{
    int i;
    int pos;
    int neg;
    int safe;

    /* positive: sum f(0..7) = 1+3+5+7+9+11+13+15 = 64 */
    pos = 0;
    for (i = 0; i < 8; i = i + 1) {
        pos = pos + f(i);
    }

    /* negative: i even → neg+=i, i odd → neg-=i; result=-3 */
    neg = 0;
    for (i = 0; i < 6; i = i + 1) {
        if ((i & 1) == 0) {
            neg = neg + i;
        } else {
            neg = neg - i;
        }
    }

    /* safety: f(2)=5>0 → safe=5 */
    if (f(2) > 0) {
        safe = 5;
    } else {
        safe = 9;
    }

    if (pos != 64) return 1;
    if (neg != -3) return 2;
    if (safe != 5) return 3;
    return 0;
}
