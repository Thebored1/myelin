type GetterMap<T extends object> = () => T;
type SetterMap<T extends object> = Partial<{ [K in keyof T]: (value: T[K]) => void }>;

/** Keeps controller state live while moving the binding surface out of the controller body. */
export function createBoundController<State extends object, Actions extends object>(
	read: GetterMap<State>,
	write: SetterMap<State>,
	actions: Actions
): State & Actions {
	return new Proxy(actions as State & Actions, {
		get(target, property, receiver) {
			if (property in target) return Reflect.get(target, property, receiver);
			return read()[property as keyof State];
		},
		set(target, property, value, receiver) {
			const setter = write[property as keyof State] as ((next: unknown) => void) | undefined;
			if (setter) {
				setter(value);
				return true;
			}
			return Reflect.set(target, property, value, receiver);
		},
		has(target, property) {
			if (property in target) return true;
			return property in read();
		},
		ownKeys(target) {
			return [...Reflect.ownKeys(target), ...Reflect.ownKeys(read())].filter(
				(key, index, keys) => keys.indexOf(key) === index
			);
		},
		getOwnPropertyDescriptor(target, property) {
			if (property in target) return Reflect.getOwnPropertyDescriptor(target, property);
			if (property in read()) {
				return { configurable: true, enumerable: true, get: () => read()[property as keyof State] };
			}
			return undefined;
		}
	}) as State & Actions;
}
