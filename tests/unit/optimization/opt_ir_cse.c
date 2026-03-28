/* opt_ir_cse.c - IR common sub-expression elimination coverage */

int status;
int a;
int b;
int guard;

int inc(int x)
{
    return x + 1;
}

void main(void)
{
    int pos;
    int neg;
    int safe;

    a = 11;
    b = 5;

    /* positive: repeated expression in same block */
    pos = (a + b) + (a + b);

    /* negative: different expression should not be merged */
    neg = (a + b) + (a - b);

    /* safety: include a call boundary in mixed expression flow */
    guard = inc(1);
    safe = (a + b) + guard;

    status = pos + neg + safe;
}
