/* sieve.c — Sieve of Eratosthenes
 *
 * Phase 1 end-to-end test for the v6c compiler.
 * Finds all primes up to SIZE using a boolean flag array.
 */

int flags[8192];
int count;

void main(void)
{
    int i;
    int k;
    int prime;
    int size;

    size = 8190;
    count = 0;

    /* initialise flags */
    i = 0;
    while (i <= size) {
        flags[i] = 1;
        i = i + 1;
    }

    /* sieve */
    i = 0;
    while (i <= size) {
        if (flags[i]) {
            prime = i + i + 3;
            k = i + prime;
            while (k <= size) {
                flags[k] = 0;
                k = k + prime;
            }
            count = count + 1;
        }
        i = i + 1;
    }

    /* count now holds the number of primes found */
}
