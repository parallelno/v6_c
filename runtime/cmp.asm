; cmp.asm — 16-bit comparison helpers for Intel 8080
;
; These are provided for convenience but the v6c code generator
; currently inlines all comparisons.  They may be used by future
; optimization passes or by hand-written assembly callers.
;
; __cmp16u:  compare HL vs DE (unsigned)
; __cmp16s:  compare HL vs DE (signed)
;
; Exit: flags set so that conditional jumps (JZ/JNZ/JC/JNC/JM/JP)
;       reflect the comparison  HL <op> DE.
; Clobbers: A, flags

; ---------------------------------------------------------------------------
; __cmp16u — unsigned compare:  sets carry if HL < DE
; ---------------------------------------------------------------------------
__cmp16u:
	MOV A,L
	SUB E
	MOV A,H
	SBB D
	RET			; carry set if HL < DE

; ---------------------------------------------------------------------------
; __cmp16s — signed compare:  sets carry if HL < DE  (signed)
;
; Strategy: XOR the sign bits. If they differ, the negative operand
; is less; if they agree, an unsigned compare suffices.
; ---------------------------------------------------------------------------
__cmp16s:
	MOV A,H
	XRA D
	JP __cmp16s_same	; same sign → unsigned compare
	; different signs: H is negative → HL < DE
	MOV A,H
	ORA A
	RM			; H negative → HL < DE (sign flag set)
	; H positive, D negative → HL >= DE
	ORA A			; clear carry
	RET
__cmp16s_same:
	MOV A,L
	SUB E
	MOV A,H
	SBB D
	RET
