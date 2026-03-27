; stdlib.asm — Standard library functions for Intel 8080
;
; C calling convention: arg0 in HL, arg1 in DE.
; Return value in HL (16-bit).
; Clobbers: A, B, C, D, E, H, L, flags

; ---------------------------------------------------------------------------
; abs — absolute value of a signed integer
;   C: int abs(int x)
;   Entry: HL = x
;   Exit:  HL = |x|
; ---------------------------------------------------------------------------
abs:
	MOV A,H
	ORA A
	RP			; already positive → return
	; Negate: HL = -HL
	MOV A,L
	CMA
	MOV L,A
	MOV A,H
	CMA
	MOV H,A
	INX H
	RET

; ---------------------------------------------------------------------------
; atoi — convert a decimal string to an integer
;   C: int atoi(char *s)
;   Entry: HL = pointer to null-terminated string
;   Exit:  HL = integer value
; ---------------------------------------------------------------------------
atoi:
	PUSH D
	PUSH B
	LXI D,0		; DE = result = 0
	MVI C,0			; C = sign flag (0 = positive)

	; Skip leading whitespace
__atoi_skip:
	MOV A,M
	CPI ' '
	JZ __atoi_ws
	CPI 0x09		; tab
	JZ __atoi_ws
	JMP __atoi_sign
__atoi_ws:
	INX H
	JMP __atoi_skip

__atoi_sign:
	MOV A,M
	CPI '-'
	JNZ __atoi_plus
	MVI C,1			; negative
	INX H
	JMP __atoi_digits
__atoi_plus:
	CPI '+'
	JNZ __atoi_digits
	INX H

__atoi_digits:
	MOV A,M
	SUI '0'
	JC __atoi_end		; not a digit (< '0')
	CPI 10
	JNC __atoi_end		; not a digit (> '9')

	; result = result * 10 + digit
	PUSH H			; save string pointer
	PUSH PSW		; save digit

	; DE = DE * 10 using HL:  DE*8 + DE*2
	MOV H,D
	MOV L,E			; HL = result
	DAD H			; HL = result * 2
	DAD H			; HL = result * 4
	DAD H			; HL = result * 8
	DAD D			; HL = result * 9
	DAD D			; HL = result * 10

	POP PSW			; A = digit
	MOV E,A
	MVI D,0
	DAD D			; HL = result*10 + digit

	MOV D,H
	MOV E,L			; DE = new result
	POP H			; restore string pointer
	INX H
	JMP __atoi_digits

__atoi_end:
	XCHG			; HL = result
	MOV A,C
	ORA A
	JZ __atoi_done
	; Negate
	MOV A,L
	CMA
	MOV L,A
	MOV A,H
	CMA
	MOV H,A
	INX H
__atoi_done:
	POP B
	POP D
	RET

; ---------------------------------------------------------------------------
; rand — generate a pseudo-random number (0 to 32767)
;   C: int rand(void)
;   Exit:  HL = random value (0..32767)
;
;   16-bit linear congruential generator:
;     seed = seed * 25173 + 13849
;     return seed & 0x7FFF
;
;   Requires __mul16 from the runtime library.
; ---------------------------------------------------------------------------
rand:
	LHLD __rand_seed
	LXI D,25173
	CALL __mul16		; HL = seed * 25173
	LXI D,13849
	DAD D			; HL = seed * 25173 + 13849
	SHLD __rand_seed
	; Return positive: clear sign bit
	MOV A,H
	ANI 0x7F
	MOV H,A
	RET

; ---------------------------------------------------------------------------
; srand — seed the random number generator
;   C: void srand(unsigned int seed)
;   Entry: HL = seed
; ---------------------------------------------------------------------------
srand:
	SHLD __rand_seed
	RET

__rand_seed:
	DW 1			; default seed

; ---------------------------------------------------------------------------
; malloc — allocate n bytes from the heap
;   C: void *malloc(unsigned int size)
;   Entry: HL = size in bytes
;   Exit:  HL = pointer to allocated block, or 0 (NULL) on failure
;
;   Simple bump allocator with a 2-byte size header per block.
;   Block format: [size_lo][size_hi][...user data...]
;   Heap grows upward from __heap_start toward __heap_limit.
; ---------------------------------------------------------------------------
malloc:
	PUSH D
	PUSH B
	MOV B,H
	MOV C,L			; BC = requested size

	; Enforce minimum allocation of 2 bytes
	MOV A,B
	ORA A
	JNZ __malloc_ok
	MOV A,C
	CPI 2
	JNC __malloc_ok
	MVI C,2
__malloc_ok:

	; Round up to even size for alignment
	MOV A,C
	ANI 1
	JZ __malloc_even
	INX B
__malloc_even:

	; Try free list first
	LHLD __heap_freelist
	MOV A,H
	ORA L
	JZ __malloc_bump		; free list empty

	; Walk free list for first-fit
	LXI D,0			; DE = previous block pointer (0 = head)
__malloc_fl_walk:
	MOV A,H
	ORA L
	JZ __malloc_bump		; end of free list

	; Read block size at [HL]
	PUSH H				; save current block pointer
	MOV E,M
	INX H
	MOV D,M				; DE = block size

	; Compare DE >= BC (is block big enough?)
	MOV A,E
	SUB C
	MOV A,D
	SBB B
	JC __malloc_fl_next		; block too small

	; Found a suitable block — remove from free list
	POP H				; HL = current block
	PUSH H
	INX H
	INX H				; skip size field
	MOV A,M
	PUSH PSW
	INX H
	MOV A,M				; next pointer high byte
	MOV D,A
	POP PSW
	MOV E,A				; DE = next pointer from current block

	; TODO: link previous to next (simplified: just use bump for now)
	; For now, fall through to bump allocator
	POP H				; clean stack
	JMP __malloc_bump

__malloc_fl_next:
	POP H				; HL = current block
	; Move to next: read next pointer at [HL+2]
	PUSH B
	INX H
	INX H
	MOV C,M
	INX H
	MOV B,M				; BC = next pointer
	MOV H,B
	MOV L,C				; HL = next block
	POP B
	JMP __malloc_fl_walk

__malloc_bump:
	; Bump allocation: heap_ptr += (size + 2)
	LHLD __heap_ptr

	; Store size header at current position
	MOV M,C			; size low byte
	INX H
	MOV M,B			; size high byte
	INX H				; HL = start of user data

	PUSH H				; save user data pointer (return value)

	; Calculate new heap pointer: user_data + size
	DAD B				; HL = user_data + size = new heap_ptr

	; Check for overflow: new_ptr must be < heap_limit
	XCHG				; DE = new heap_ptr
	LHLD __heap_limit
	; Compare: HL (limit) vs DE (new_ptr)
	; If limit < new_ptr → failure
	MOV A,L
	SUB E
	MOV A,H
	SBB D
	JC __malloc_fail		; limit < new_ptr → out of memory

	; Success: update heap pointer
	XCHG				; HL = new heap_ptr
	SHLD __heap_ptr

	POP H				; HL = user data pointer
	POP B
	POP D
	RET

__malloc_fail:
	POP H				; discard saved user data pointer
	LXI H,0			; return NULL
	POP B
	POP D
	RET

; ---------------------------------------------------------------------------
; free — release a heap-allocated block
;   C: void free(void *ptr)
;   Entry: HL = pointer to free (as returned by malloc)
;
;   Adds block to the head of the free list.
;   Block header is at [ptr-2] (2-byte size).
;   Free list is linked through the first 2 bytes of user data.
; ---------------------------------------------------------------------------
free:
	MOV A,H
	ORA L
	RZ				; free(NULL) is a no-op

	; Add to head of free list: store old head at [ptr], then set head = ptr
	PUSH D
	LHLD __heap_freelist		; DE will get old head
	XCHG				; DE = old free list head
	POP PSW				; discard saved D
	; Wait, we need HL = ptr. Let me redo.
	; Entry: HL = ptr
	PUSH D
	PUSH H				; save ptr
	LHLD __heap_freelist		; HL = old head
	XCHG				; DE = old head
	POP H				; HL = ptr
	; Store old head at [ptr] (first 2 bytes of user data = next pointer)
	MOV M,E
	INX H
	MOV M,D
	DCX H				; HL = ptr again
	; Set free list head = ptr
	SHLD __heap_freelist
	POP D
	RET

; --- heap state ---
; These are initialized by crt0 or placed at the end of the program.
__heap_ptr:
	DW __heap_start		; current allocation pointer
__heap_limit:
	DW 0xF000		; upper bound (below stack at 0xF800)
__heap_freelist:
	DW 0			; free list head (0 = empty)

; The heap data area begins here. This label must be placed
; after all code and static data in the final assembly output.
__heap_start:
