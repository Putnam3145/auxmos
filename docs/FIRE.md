Auxmos has its own system for fires, which tries to replicate many of the original features of plasma/trit fires that Citadel had at the time of implementation (circa 2019 TG, essentially) but is different in some key ways. Here are the important parts you need to know:

# Disabling

You can disable generic fires entirely by simply setting the reaction to `exclude = TRUE` *or* by compiling without the `generic_fires` hook, using the other hooks instead.

# Balance concerns

## Fire danger

The hottest fires using the "default" values for things--the one Citadel uses right now--is plasma fires, specifically ones that are *not* oxygen-rich. Trtiium generation sucks a lot of the heat out of it, but that heat is put right back in when the tritium itself burns. This is liable to be unintuitive to many, but it's... probably fine? At the heat scales we're talking, all fires are *essentially* identically dangerous anyway, i.e. "don't go there". Plasma fires are, I would say, hotter than they were before on average, but in oxygen-rich environments less hot, because trit fires were stupid.

## Tritium fires

Tritium fires are now an ordinary kind of fire. They're hotter than hydrogen fires, if done on their own, and release radiation, but they don't have weird properties like eating all of the oxygen out of the air ASAP. In fact, they're the *least* oxygen-hungry fire now (tied with hydrogen), which should be considered.

## Bombs

The strongest bombs are tritium. This has always been true, even though trit fires are less hot than plasma fires, because tritium has a very, very low heat capacity and trit fires are fuel rich. The new ideal bomb fuel mix is 2 : 1 tritium to oxygen. I have personally been able to get up to 5000 shockwave radius with such a bomb using hyper-noblium as the hot side; this is about equal to the old "normal" trit bomb. Balancing should be done accordingly. I changed the formula to have an explicit "baseline" where you get 50,000 points, a sharp dropoff before that point and asymptotically approaching 100,000 points as you get higher and higher, for this purpose.

## Heat cans

The new ideal plasma/oxy ratio for maximum heat is 1.4 : 1. This is 58.333...% plasma. Not much else to say about that.

## Newly flammable gases

You can make any gas flammable by just changing its properties. By default, on Citadel, nitrous oxide and nitrogen dioxide ("nitryl") can be used as oxidizers, and nitrogen is "flammable" at high temperatures (but the reaction actually *reduces* the total heat). Pluoxium is an oxidizer that reduces the fire's heat a good deal and eats a lot of the fuel. All of this is byond-side and can be taken or left as desired.

## Speed

Auxmos is fast. Very, very fast. Generic fires are balanced for this; default plasma fires and trit fires are not. Auxmos will make fires seem to burn much, much hotter than normal. This is not actually the case: they're burning just as hot, they're just going way, way faster. There's a constant in auxmos that determines what the maximum amount of fuel that can be burned in a fire in one tick is: right now it's 20%, specifically to avoid fires burning too hot too fast. I'm not sure it's even enough, ha.
