; mul32.asm — 32-bit unsigned multiply for Intel 8080
;
; Entry:  __op1 (4 bytes) = multiplicand
;         __op2 (4 bytes) = multiplier
; Exit:   __op1 (4 bytes) = result (low 32 bits)
;         HL = low 16 bits of result
; Clobbers: A, B, C, D, E, flags

__mul32:
	; result in __res32 = 0
	XRA A
	STA __res32
	STA __res32+1
	STA __res32+2
	STA __res32+3
	; Loop terminates when the multiplier (__op2) becomes zero after
	; repeated right-shifting.  The early-exit check avoids unnecessary
	; iterations when the multiplier has few set bits.
__mul32_loop:
	; test low bit of multiplier (__op2)
	LDA __op2
	RAR
	JNC __mul32_skip
	; result += multiplicand
	LDA __res32
	MOV C,A
	LDA __op1
	ADD C
	STA __res32
	LDA __res32+1
	MOV C,A
	LDA __op1+1
	ADC C
	STA __res32+1
	LDA __res32+2
	MOV C,A
	LDA __op1+2
	ADC C
	STA __res32+2
	LDA __res32+3
	MOV C,A
	LDA __op1+3
	ADC C
	STA __res32+3
__mul32_skip:
	; shift multiplicand left (__op1 <<= 1)
	LDA __op1
	ADD A
	STA __op1
	LDA __op1+1
	ADC A
	STA __op1+1
	LDA __op1+2
	ADC A
	STA __op1+2
	LDA __op1+3
	ADC A
	STA __op1+3
	; shift multiplier right (__op2 >>= 1)
	LDA __op2+3
	ORA A
	RAR
	STA __op2+3
	LDA __op2+2
	RAR
	STA __op2+2
	LDA __op2+1
	RAR
	STA __op2+1
	LDA __op2
	RAR
	STA __op2
	; check if multiplier is zero
	ORA A
	JNZ __mul32_loop
	LDA __op2+1
	ORA A
	JNZ __mul32_loop
	LDA __op2+2
	ORA A
	JNZ __mul32_loop
	LDA __op2+3
	ORA A
	JNZ __mul32_loop
	; copy result back to __op1
	LDA __res32
	STA __op1
	LDA __res32+1
	STA __op1+1
	LDA __res32+2
	STA __op1+2
	LDA __res32+3
	STA __op1+3
	; return low 16 bits in HL
	LHLD __op1
	RET

__res32:
	.storage 4		; 32-bit result accumulator
