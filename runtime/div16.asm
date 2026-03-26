; div16.asm — 16-bit unsigned and signed divide/modulo for Intel 8080
;
; __div16u:  HL = HL / DE  (unsigned)
; __mod16u:  HL = HL % DE  (unsigned)
; __div16s:  HL = HL / DE  (signed)
; __mod16s:  HL = HL % DE  (signed)
;
; Clobbers: A, B, C, flags

; ---------------------------------------------------------------------------
; __div16u — unsigned 16-bit divide:  HL = HL / DE
; ---------------------------------------------------------------------------
__div16u:
	CALL __divmod16u	; BC = quotient, HL = remainder
	MOV H,B
	MOV L,C
	RET

; ---------------------------------------------------------------------------
; __mod16u — unsigned 16-bit modulo:  HL = HL % DE
; ---------------------------------------------------------------------------
__mod16u:
	CALL __divmod16u	; BC = quotient, HL = remainder
	RET

; ---------------------------------------------------------------------------
; __div16s — signed 16-bit divide:  HL = HL / DE
; ---------------------------------------------------------------------------
__div16s:
	MOV A,H
	XRA D
	PUSH PSW		; save result sign (bit 7)
	MOV A,H
	ORA A
	JP __div16s_pn
	MOV A,L
	CMA
	MOV L,A
	MOV A,H
	CMA
	MOV H,A
	INX H
__div16s_pn:
	MOV A,D
	ORA A
	JP __div16s_pd
	MOV A,E
	CMA
	MOV E,A
	MOV A,D
	CMA
	MOV D,A
	INX D
__div16s_pd:
	CALL __divmod16u
	MOV H,B
	MOV L,C
	POP PSW
	ORA A
	RP
	MOV A,L
	CMA
	MOV L,A
	MOV A,H
	CMA
	MOV H,A
	INX H
	RET

; ---------------------------------------------------------------------------
; __mod16s — signed 16-bit modulo:  HL = HL % DE
; ---------------------------------------------------------------------------
__mod16s:
	MOV A,H
	PUSH PSW		; save dividend sign
	ORA A
	JP __mod16s_pn
	MOV A,L
	CMA
	MOV L,A
	MOV A,H
	CMA
	MOV H,A
	INX H
__mod16s_pn:
	MOV A,D
	ORA A
	JP __mod16s_pd
	MOV A,E
	CMA
	MOV E,A
	MOV A,D
	CMA
	MOV D,A
	INX D
__mod16s_pd:
	CALL __divmod16u
	POP PSW
	ORA A
	RP
	MOV A,L
	CMA
	MOV L,A
	MOV A,H
	CMA
	MOV H,A
	INX H
	RET

; ---------------------------------------------------------------------------
; __divmod16u — core unsigned 16-bit restoring division
;
;   Entry: HL = dividend, DE = divisor
;   Exit:  BC = quotient, HL = remainder
;   Clobbers: A, flags
;
;   Algorithm: standard restoring division, 16 iterations.
;   Divisor stored in memory to free DE for the dividend shift register.
;   BC temporarily reused for the trial subtraction (saved/restored via
;   PUSH/POP each iteration).
; ---------------------------------------------------------------------------
__divmod16u:
	MOV A,D
	ORA E
	JNZ __dm16_go
	LXI B,0
	LXI H,0
	RET			; div by zero → 0
__dm16_go:
	; save divisor to memory
	MOV A,E
	STA __dm16_dv
	MOV A,D
	STA __dm16_dv+1
	; DE = dividend, HL = remainder = 0, BC = quotient = 0
	XCHG
	LXI H,0
	LXI B,0
	MVI A,16
	STA __dm16_cnt
__dm16_loop:
	; quotient <<= 1
	MOV A,C
	ADD A
	MOV C,A
	MOV A,B
	ADC A
	MOV B,A
	; shift MSB of dividend (DE) into remainder (HL)
	MOV A,E
	ADD A
	MOV E,A
	MOV A,D
	ADC A
	MOV D,A		; DE <<= 1, carry = old MSB
	MOV A,L
	RAL
	MOV L,A
	MOV A,H
	RAL
	MOV H,A		; HL = (HL << 1) | carry
	; trial subtract: remainder (HL) - divisor
	PUSH H		; save remainder in case it doesn't fit
	PUSH B		; save quotient (free BC for divisor)
	LDA __dm16_dv
	MOV C,A
	LDA __dm16_dv+1
	MOV B,A		; BC = divisor
	MOV A,L
	SUB C
	MOV L,A
	MOV A,H
	SBB B
	MOV H,A		; HL -= divisor (trial)
	POP B		; restore quotient
	JC __dm16_nfit
	; fits: keep subtracted HL, set quotient bit 0
	POP PSW		; discard saved remainder
	INR C		; quotient |= 1
	JMP __dm16_next
__dm16_nfit:
	; doesn't fit: restore old remainder
	POP H
__dm16_next:
	LDA __dm16_cnt
	DCR A
	STA __dm16_cnt
	JNZ __dm16_loop
	; BC = quotient, HL = remainder
	RET

__dm16_dv:
	DS 2		; divisor temp
__dm16_cnt:
	DS 1		; loop counter
