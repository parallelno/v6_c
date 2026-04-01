/* opt_ir_narrow.c - IR byte-range narrowing coverage */

unsigned char a;
unsigned char b;
unsigned char c;
unsigned int ux;
unsigned int uy;

int main(void)
{
    int pos;
    int neg;
    int safe;

    /* positive: 200+17=217 (fits unsigned char) */
    a = 200;
    b = 17;
    c = a + b;
    pos = c;

    /* negative: -1000/3 = -333 */
    neg = (-1000) / 3;

    /* safety: 250/7=35 remainder 5; safe=40 */
    ux = 250;
    uy = 7;
    safe = (int)(ux / uy);
    safe = safe + (int)(ux % uy);

    if (pos != 217) return 1;
    if (neg != -333) return 2;
    if (safe != 40) return 3;
    return 0;
}
