/* opt_ir_narrow.c - IR byte-range narrowing coverage */

int status;
unsigned char a;
unsigned char b;
unsigned char c;
unsigned int ux;
unsigned int uy;
int pos;
int neg;
int safe;

void main(void)
{
    /* positive: byte-proven arithmetic */
    a = 200;
    b = 17;
    c = a + b;
    pos = c;

    /* negative: full-width signed math path */
    neg = (-1000) / 3;

    /* safety: unsigned narrow-friendly div/mod */
    ux = 250;
    uy = 7;
    safe = (int)(ux / uy);
    safe = safe + (int)(ux % uy);

    status = pos + neg + safe;
}
