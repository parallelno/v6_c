
; it advances a reg by the diff equals to (addr_to - addr_from)
; if a diff is inside of [-2, 2], used: INR_*(N)/DCR_*(N). 8-16cc
; if a diff is outside of [-2, 2], used: mov a, NN; add reg; mov reg, a. 20cc
; supports runtime data that's fit inside one $100 block: backs_runtime_data, overlays_runtime_data
; use:
; reg, a
.macro C_ADVANCE(addr_from, addr_to, reg_a = NULL)
		diff_addr = addr_to - addr_from
		mvi_val .var diff_addr
		.if mvi_val < 0
			mvi_val = $ffff + mvi_val + 1
		.endif

		.if reg_a == BY_A
			mvi a, <mvi_val
			add c
			mov c, a
			; validation
			.if diff_addr >= -2 && diff_addr <= 2
				.error "C_ADVANCE(" addr_from ", " addr_to", BY_A) with diff (" diff_addr ") is in too short range [-2, 2]. Keep the third argument undefined."
			.endif
		.endif

		.if reg_a == NULL
			.if diff_addr > 0
				INR_C(diff_addr)
			.endif
			.if diff_addr < 0
				DCR_C(-diff_addr)
			.endif
			; validation
			.if diff_addr < -2 || diff_addr > 2
				.error "C_ADVANCE(" addr_from ", " addr_to") with diff (" diff_addr ") is outside [-2, 2] range. Use BY_A for the third argument."
			.endif
		.endif

		.if (>addr_from != >backs_runtime_data && >addr_from != >overlays_runtime_data ) || (>addr_to != >backs_runtime_data && >addr_to != >overlays_runtime_data)
			.error "C_ADVANCE(" addr_from ", " addr_to", "reg_a") with diff (" diff_addr ") is designed only for the runtime data fit into one $100 block (backs_runtime_data and overlays_runtime_data). For wider runtimedata use HL_ADVANCE instead."
		.endif
		.if reg_a != NULL && reg_a != BY_A
			.error "C_ADVANCE(" addr_from ", " addr_to") with diff (" diff_addr ") accepts only NULL and BY_A for the third argument."
		.endif
.endmacro

.macro E_ADVANCE(addr_from, addr_to, reg_a = NULL)
		diff_addr = addr_to - addr_from
		mvi_val .var diff_addr
		.if mvi_val < 0
			mvi_val = $ffff + mvi_val + 1
		.endif

		.if reg_a == BY_A
			mvi a, <mvi_val
			add e
			mov e, a
			; validation
			.if diff_addr >= -2 && diff_addr <= 2
				.error "E_ADVANCE(" addr_from ", " addr_to", BY_A) with diff (" diff_addr ") is in too short range [-2, 2]. Keep the third argument undefined."
			.endif
		.endif

		.if reg_a == NULL
			.if diff_addr > 0
				INR_E(diff_addr)
			.endif
			.if diff_addr < 0
				DCR_E(-diff_addr)
			.endif
			; validation
			.if diff_addr < -2 || diff_addr > 2
				.error "E_ADVANCE(" addr_from ", " addr_to") with diff (" diff_addr ") is outside [-2, 2] range. Use BY_A for the third argument."
			.endif
		.endif

		.if (>addr_from != >backs_runtime_data && >addr_from != >overlays_runtime_data ) || (>addr_to != >backs_runtime_data && >addr_to != >overlays_runtime_data)
			.error "E_ADVANCE(" addr_from ", " addr_to", "reg_a") with diff (" diff_addr ") is designed only for the runtime data fit into one $100 block (backs_runtime_data and overlays_runtime_data). For wider runtimedata use HL_ADVANCE instead."
		.endif
		.if reg_a != NULL && reg_a != BY_A
			.error "E_ADVANCE(" addr_from ", " addr_to") with diff (" diff_addr ") accepts only NULL and BY_A for the third argument."
		.endif
.endmacro

.macro L_ADVANCE(addr_from, addr_to, reg_a = NULL)
		diff_addr = addr_to - addr_from
		mvi_val .var diff_addr
		.if mvi_val < 0
			mvi_val = $ffff + mvi_val + 1
		.endif

		.if reg_a == BY_A
			mvi a, <mvi_val
			add l
			mov l, a
			; validation
			.if diff_addr >= -2 && diff_addr <= 2
				.error "L_ADVANCE(" addr_from ", " addr_to", BY_A) with diff (" diff_addr ") is in too short range [-2, 2]. Use HL_ADVANCE without third argument."
			.endif
		.endif

		.if reg_a == NULL
			.if diff_addr > 0
				INR_L(diff_addr)
			.endif
			.if diff_addr < 0
				DCR_L(-diff_addr)
			.endif
			; validation
			.if diff_addr < -2 || diff_addr > 2
				.error "L_ADVANCE(" addr_from ", " addr_to") with diff (" diff_addr ") is outside [-2, 2] range. Use BY_A for the third argument."
			.endif
		.endif

		.if (>addr_from != >backs_runtime_data && >addr_from != >overlays_runtime_data ) || (>addr_to != >backs_runtime_data && >addr_to != >overlays_runtime_data)
			.error "L_ADVANCE(" addr_from ", " addr_to", "reg_a") with diff (" diff_addr ") is designed only for the runtime data fit into one $100 block (backs_runtime_data and overlays_runtime_data). For wider runtimedata use HL_ADVANCE instead."
		.endif
		.if reg_a != NULL && reg_a != BY_A
			.error "L_ADVANCE(" addr_from ", " addr_to") with diff (" diff_addr ") accepts only NULL and BY_A for the third argument."
		.endif
.endmacro
