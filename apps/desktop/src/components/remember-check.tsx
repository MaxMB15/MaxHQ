// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The box on a card that asks for your consent that keeps your answer.
// Once it's ticked, a line under it says where in Settings to change it.

import { Checkbox } from "@/components/ui/checkbox";

export const RememberCheck = ({
	label,
	where,
	checked,
	onChange,
	disabled,
}: {
	label: string;
	/** Says where in Settings to change the kept answer, shown once it's ticked. */
	where: string;
	checked: boolean;
	onChange: (checked: boolean) => void;
	disabled?: boolean;
}) => (
	<div className="flex flex-col gap-1">
		<label className="flex w-fit items-center gap-2 text-[12px] text-foreground">
			<Checkbox
				aria-label={label}
				checked={checked}
				disabled={disabled}
				onCheckedChange={(on) => onChange(on)}
			/>
			{label}
		</label>
		{checked ? (
			<p role="status" className="ml-6 text-[11.5px] text-muted-foreground">
				{where}
			</p>
		) : null}
	</div>
);
