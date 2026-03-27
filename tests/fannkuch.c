/* fannkuch.c - Fannkuch benchmark for v6c
 *
 * Computes the maximum number of prefix flips needed for permutations
 * of [0..n-1]. Result is written to global `fannkuch_result`.
 */

int perm[8];
int perm1[8];
int count[8];
int fannkuch_result;

void main(void)
{
    int n;
    int r;
    int i;
    int k;
    int flips;
    int maxflips;
    int permcount;
    int first;
    int last;
    int t;

    n = 7;
    maxflips = 0;
    permcount = 0;

    i = 0;
    while (i < n) {
        perm1[i] = i;
        i = i + 1;
    }

    r = n;

    while (1) {
        while (r != 1) {
            count[r - 1] = r;
            r = r - 1;
        }

        i = 0;
        while (i < n) {
            perm[i] = perm1[i];
            i = i + 1;
        }

        flips = 0;
        while (1) {
            first = perm[0];
            if (first == 0) {
                break;
            }

            last = first;
            i = 0;
            k = last;
            while (i < k) {
                t = perm[i];
                perm[i] = perm[k];
                perm[k] = t;
                i = i + 1;
                k = k - 1;
            }
            flips = flips + 1;
        }

        if (flips > maxflips) {
            maxflips = flips;
        }

        while (1) {
            if (r == n) {
                fannkuch_result = maxflips + permcount;
                return;
            }

            t = perm1[0];
            i = 0;
            while (i < r - 1) {
                perm1[i] = perm1[i + 1];
                i = i + 1;
            }
            perm1[r - 1] = t;

            count[r - 1] = count[r - 1] - 1;
            if (count[r - 1] > 0) {
                break;
            }

            r = r + 1;
        }

        permcount = permcount + 1;
    }
}
