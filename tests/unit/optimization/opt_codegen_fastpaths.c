/* opt_codegen_fastpaths.c - codegen helper fast-path coverage */

int main(void)
{
    int x;
    int pos;
    int neg;
    int safe;

    x = 9;

    /* positive: mul/div/mod/shift constants with known fast paths */
    pos = (x * 2) + (x * 3) + (x * 4) + (x * 8);  /* 18+27+36+72=153 */
    pos = pos + (x / 1);   /* +9=162 */
    pos = pos + (x % 1);   /* +0=162 */
    pos = pos + (x % 2);   /* +1=163 */
    pos = pos + (x << 1);  /* +18=181 */
    pos = pos + (x >> 1);  /* +4=185 */

    /* negative: generic cases: 9*7+9/3+9%3 = 63+3+0 = 66 */
    neg = (x * 7) + (x / 3) + (x % 3);

    /* safety: preserve signed behavior: -33/3 = -11 */
    safe = (-33) / 3;

    if (pos != 185) return 1;
    if (neg != 66) return 2;
    if (safe != -11) return 3;
    return 0;
}
