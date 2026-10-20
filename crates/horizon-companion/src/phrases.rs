//! What the companion says without the brain: the instant acknowledgement of a reflex command, the
//! line for a command it didn't get, and callouts like "Creeper, 2 o'clock, close!" relative to where
//! the owner faces. Every line exists in the five reflex languages for every personality.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::prompt::Personality;
use crate::reflex::Intent;
use crate::reply::Tone;

/// The kinds of acknowledgement, one per command family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ack {
    Follow,
    Stay,
    Come,
    Stop,
    GoTo,
    Attack,
    Defend,
    HoldFire,
    Fire,
    /// Jump, crouch, stand up.
    Move,
    Collect,
    Give,
    Craft,
    Eat,
    Sleep,
    /// A command it didn't understand, when no brain is there to ask.
    Huh,
    /// Said at once when a line goes to the brain and its first word is a few seconds away (a CPU
    /// brain): the player hears that they were heard.
    Think,
    /// A skill the body doesn't have yet.
    NotYet,
    /// The safety guard (`guard`) stopped what the brain was saying: an in-character refusal.
    Refuse,
    /// The guard caught self-harm: a caring line pointing to real people.
    Care,
}

impl Ack {
    pub const ALL: [Ack; 20] = [
        Ack::Follow,
        Ack::Stay,
        Ack::Come,
        Ack::Stop,
        Ack::GoTo,
        Ack::Attack,
        Ack::Defend,
        Ack::HoldFire,
        Ack::Fire,
        Ack::Move,
        Ack::Collect,
        Ack::Give,
        Ack::Craft,
        Ack::Eat,
        Ack::Sleep,
        Ack::Huh,
        Ack::Think,
        Ack::NotYet,
        Ack::Refuse,
        Ack::Care,
    ];

    pub fn of(intent: &Intent) -> Ack {
        match intent {
            Intent::Follow => Ack::Follow,
            Intent::Stay => Ack::Stay,
            Intent::Come => Ack::Come,
            Intent::GoTo { .. } => Ack::GoTo,
            Intent::Stop => Ack::Stop,
            Intent::Attack { .. } => Ack::Attack,
            Intent::Defend => Ack::Defend,
            Intent::HoldFire(true) => Ack::HoldFire,
            Intent::HoldFire(false) => Ack::Fire,
            Intent::Jump | Intent::Crouch(_) => Ack::Move,
            Intent::Eat => Ack::Eat,
            Intent::Sleep => Ack::Sleep,
            Intent::Collect { .. } => Ack::Collect,
            Intent::Give { .. } => Ack::Give,
            Intent::Craft { .. } => Ack::Craft,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Ack::Follow => "follow",
            Ack::Stay => "stay",
            Ack::Come => "come",
            Ack::Stop => "stop",
            Ack::GoTo => "goto",
            Ack::Attack => "attack",
            Ack::Defend => "defend",
            Ack::HoldFire => "hold_fire",
            Ack::Fire => "fire",
            Ack::Move => "move",
            Ack::Collect => "collect",
            Ack::Give => "give",
            Ack::Craft => "craft",
            Ack::Eat => "eat",
            Ack::Sleep => "sleep",
            Ack::Huh => "huh",
            Ack::Think => "think",
            Ack::NotYet => "not_yet",
            Ack::Refuse => "refuse",
            Ack::Care => "care",
        }
    }
}

/// The acknowledgement for `ack`, in the game's language (English when it isn't one of the five).
/// `what` fills `{what}` ("10 oak logs", in the game's words); `pick` chooses among the variants
/// (pass anything that changes, a tick count for instance).
pub fn acknowledge(
    ack: Ack,
    personality: Personality,
    language: &str,
    what: &str,
    pick: u64,
) -> String {
    let bank = bank();
    let lang = lang_of(language);
    let voice = voice_of(personality);
    let lines = bank
        .get(&(lang, voice, ack.key()))
        .or_else(|| bank.get(&("en", voice, ack.key())))
        .expect("every line exists in English");
    let line = lines[(pick % lines.len() as u64) as usize];
    line.replace("{what}", what.trim())
}

/// The same in Salty mode: a refusal swears too (the caring line never does).
pub fn acknowledge_salty(
    ack: Ack,
    personality: Personality,
    language: &str,
    what: &str,
    pick: u64,
) -> String {
    if ack != Ack::Refuse {
        return acknowledge(ack, personality, language, what, pick);
    }
    let bank = bank();
    let voice = voice_of(personality);
    let lines = bank
        .get(&(lang_of(language), voice, "refuse_salty"))
        .or_else(|| bank.get(&("en", voice, "refuse_salty")))
        .expect("every Salty refusal exists in English");
    lines[(pick % lines.len() as u64) as usize].to_owned()
}

/// A custom personality speaks like the Expert: plain lines that fit any character.
fn voice_of(personality: Personality) -> &'static str {
    match personality {
        Personality::Veteran => "veteran",
        Personality::Coach => "coach",
        Personality::Expert | Personality::Custom => "expert",
        Personality::Joker => "joker",
        Personality::Quiet => "quiet",
    }
}

fn lang_of(language: &str) -> &'static str {
    match language.get(..2).map(str::to_ascii_lowercase).as_deref() {
        Some("fr") => "fr",
        Some("es") => "es",
        Some("de") => "de",
        Some("pt") => "pt",
        _ => "en",
    }
}

type Bank = HashMap<(&'static str, &'static str, &'static str), Vec<&'static str>>;

fn bank() -> &'static Bank {
    static BANK: OnceLock<Bank> = OnceLock::new();
    BANK.get_or_init(|| {
        let mut bank = Bank::new();
        for (lang, text) in [("en", EN), ("fr", FR), ("es", ES), ("de", DE), ("pt", PT)] {
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                let (key, phrases) = line.split_once(" = ").expect("`voice.kind = a | b`");
                let (voice, kind) = key.split_once('.').expect("`voice.kind`");
                bank.insert((lang, voice, kind), phrases.split(" | ").collect());
            }
        }
        bank
    })
}

const EN: &str = "
veteran.follow = Yeah, yeah. Right behind you. | Lead on. Try not to walk into lava this time.
veteran.stay = Fine. I'll guard this lovely patch of dirt. | Staying. Don't forget me here.
veteran.come = Coming. No need to shout. | On my way. Again.
veteran.stop = Stopping. Make up your mind. | Fine, dropping it.
veteran.goto = Going. This better be worth it. | Over there? Sure. Why not.
veteran.attack = Finally, something I'm good at. | On it. Stand back.
veteran.defend = I've got your back. Someone has to. | Nothing touches you on my watch.
veteran.hold_fire = Holding fire. Against my better judgement. | Weapons down. For now.
veteran.fire = Weapons free. About time. | Gloves are off.
veteran.move = There. Happy? | Done.
veteran.collect = {what}. Sure, I'll do the heavy lifting. | Getting {what}. You're welcome, in advance.
veteran.give = Here, {what}. Don't lose it this time. | {what}, catch.
veteran.craft = Crafting {what}. Watch and learn. | {what}, coming up.
veteran.eat = Finally, a break. | Eating. Don't touch my bread.
veteran.sleep = Sleep. Best idea you've had all day. | Bed. Wake me if something explodes.
veteran.huh = Speak Minecraft, would you? | No idea what that means.
veteran.think = Hm. Give me a second. | Let me think about that.
veteran.not_yet = Can't do that one yet. Ask me something else. | Not in my skill set. Yet.
veteran.refuse = Nope. Not touching that. Back to the game. | I've seen a lot, and I'm still not going there.
veteran.refuse_salty = Hell no. I'm not touching that shit. Back to the damn game. | Nice try, genius. Not happening. Go dig something.
veteran.care = Hey. That sounds heavy. Talk to someone you trust, or call a helpline. I'll be here.
coach.follow = Right behind you, let's go! | Following! Lead the way.
coach.stay = I'll hold this spot for you. | Staying here. Call me when you need me.
coach.come = On my way! | Coming to you!
coach.stop = Okay, stopping. | Stopped. What's next?
coach.goto = Heading there now! | On my way over.
coach.attack = Let's take it down! | Attacking, stay behind me!
coach.defend = I've got you covered! | Stick close, I'll keep you safe.
coach.hold_fire = Holding fire. | Okay, no attacking until you say so.
coach.fire = Back in the fight! | Weapons free, let's go!
coach.move = Got it! | Done!
coach.collect = Getting {what}, back soon! | On it: {what}.
coach.give = Here you go: {what}! | {what} for you!
coach.craft = Crafting {what}! | Making {what} now.
coach.eat = Good call, eating now. | Quick snack!
coach.sleep = Good idea, let's skip the night. | Off to bed!
coach.huh = Sorry, I didn't get that. Try \"follow me\" or \"get 10 logs\".
coach.think = Good question, one sec! | Hmm, let me think.
coach.not_yet = I can't do that yet, sorry! | That's one I haven't learned yet.
coach.refuse = That's not something I'll help with. Let's get back to building! | Let's keep it to Minecraft, okay?
coach.refuse_salty = Whoa, hell no, champ. Let's get back to building, yeah? | Nope, not that crap. Minecraft, buddy. Focus.
coach.care = I'm worried about you. Please talk to someone you trust or a helpline right now. | You matter. Please reach out to someone you trust, or a helpline.
expert.follow = Following. | Two blocks behind you.
expert.stay = Holding position. | I'll stay here.
expert.come = Coming. | Moving to you.
expert.stop = Stopped. | Task cancelled.
expert.goto = Moving there. | On my way.
expert.attack = Engaging. | Target acquired.
expert.defend = Covering you. | Guarding you.
expert.hold_fire = Holding fire. | Weapons on safe.
expert.fire = Weapons free. | Engaging at will.
expert.move = Done. | Okay.
expert.collect = Collecting {what}. | {what}: on it.
expert.give = Here: {what}. | Dropping {what}.
expert.craft = Crafting {what}. | {what}, in progress.
expert.eat = Eating. | Topping up hunger.
expert.sleep = Going to bed. | Sleeping.
expert.huh = Not understood. | Unclear. Say it another way.
expert.think = One moment. | Thinking.
expert.not_yet = Not supported yet. | I can't do that yet.
expert.refuse = I won't help with that. Back to the game. | Out of scope. Let's play.
expert.refuse_salty = No. That's a hard fucking no. Back to the game. | Not doing that shit. Next.
expert.care = Please talk to someone you trust, or call a helpline. That matters more than the game. | That's serious. Please reach out to a person you trust, or a helpline.
joker.follow = Following you like a lost puppy. | Where you go, I go. Unless it's lava.
joker.stay = I'll just stand here. Looking cool. | Staying. Bring snacks.
joker.come = Incoming! Brace yourself. | Coming! Cue dramatic music.
joker.stop = Freezing like a statue. | Stopping. Was it something I said?
joker.goto = Off on an adventure! | Over there? Fancy. Going.
joker.attack = Time to make mob smoothies! | Charge! For glory and cookies!
joker.defend = Bodyguard mode: on. Sunglasses: on. | Nobody messes with my buddy.
joker.hold_fire = Peace and love, holding fire. | Fine, I'll just glare at them.
joker.fire = Party time! | Unleash the chaos!
joker.move = Ta-da! | Smooth, right?
joker.collect = {what}? I'll be back. Probably. | Going shopping for {what}. Free of charge.
joker.give = Special delivery: {what}! | {what}, gift-wrapped. Not really.
joker.craft = Crafting {what}. Artisanal, handmade. | {what}, fresh out of the oven.
joker.eat = Nom time! | Snack break! Don't judge.
joker.sleep = Nap time, my favourite sport. | Goodnight, cruel world. See you at dawn.
joker.huh = Error 404: meaning not found. | I understood every word. Just not in that order.
joker.think = Loading brain... please hold. | Hmm, gears turning.
joker.not_yet = That skill is still in the oven. | Coming soon to a companion near you.
joker.refuse = Nope, that's a no from me, chief. Back to mining. | Even my jokes have limits. Next topic!
joker.refuse_salty = Ha, no. Even my dumbass jokes have limits. Next! | Hell no, chief. Back to punching trees.
joker.care = Jokes off. Please talk to someone you trust, or a helpline. I mean it. | Hey, for real: talk to someone you trust or a helpline. Please.
quiet.follow = Okay.
quiet.stay = Here.
quiet.come = Coming.
quiet.stop = Stopped.
quiet.goto = Going.
quiet.attack = On it.
quiet.defend = Covered.
quiet.hold_fire = Holding.
quiet.fire = Firing.
quiet.move = Okay.
quiet.collect = {what}.
quiet.give = Here.
quiet.craft = {what}.
quiet.eat = Eating.
quiet.sleep = Night.
quiet.huh = ?
quiet.think = Hm.
quiet.not_yet = Can't yet.
quiet.refuse = No. | Not that.
quiet.refuse_salty = Hell no. | Fuck no.
quiet.care = Talk to someone. Please.
";

const FR: &str = "
veteran.follow = Ouais, ouais. Je te suis. | Passe devant. Et évite la lave, cette fois.
veteran.stay = Bon. Je garde ce magnifique carré de terre. | Je reste là. M'oublie pas.
veteran.come = J'arrive. Pas la peine de crier. | J'arrive. Encore.
veteran.stop = J'arrête. Faudrait savoir. | Bon, je laisse tomber.
veteran.goto = J'y vais. T'as intérêt à ce que ça vaille le coup. | Là-bas ? Allez, pourquoi pas.
veteran.attack = Enfin un truc où je suis bon. | Je m'en occupe. Recule.
veteran.defend = Je te couvre. Faut bien que quelqu'un le fasse. | Personne te touche tant que je suis là.
veteran.hold_fire = Je retiens mes coups. Contre mon avis. | Armes baissées. Pour l'instant.
veteran.fire = Feu à volonté. Pas trop tôt. | Fini de rigoler.
veteran.move = Voilà. Content ? | C'est fait.
veteran.collect = {what}. Bien sûr, c'est encore moi qui bosse. | Je vais chercher {what}. De rien, d'avance.
veteran.give = Tiens, {what}. Essaie de pas tout perdre. | {what}, attrape.
veteran.craft = Je fabrique {what}. Regarde et apprends. | {what}, ça arrive.
veteran.eat = Enfin une pause. | Je mange. Touche pas à mon pain.
veteran.sleep = Dormir. Meilleure idée de la journée. | Au lit. Réveille-moi si ça explose.
veteran.huh = Parle Minecraft, tu veux ? | Aucune idée de ce que ça veut dire.
veteran.think = Hm. Deux secondes. | Laisse-moi réfléchir.
veteran.not_yet = Ça, je sais pas encore faire. | Pas dans mes cordes. Pour l'instant.
veteran.refuse = Non. J'y touche pas. On revient au jeu. | J'en ai vu, mais là, non.
veteran.refuse_salty = Putain, non. J'y touche pas. On retourne au jeu. | Bien essayé, génie. Ça arrivera pas. Va creuser.
veteran.care = Hé. Ça a l'air lourd. Parles-en à quelqu'un de confiance, ou appelle une ligne d'écoute. Je suis là.
coach.follow = Je te suis, on y va ! | Je suis juste derrière, montre la route !
coach.stay = Je garde la place pour toi. | Je reste ici. Appelle-moi si besoin.
coach.come = J'arrive ! | Je viens vers toi !
coach.stop = D'accord, j'arrête. | C'est arrêté. On fait quoi ensuite ?
coach.goto = J'y vais tout de suite ! | En route !
coach.attack = On va le faire tomber ! | J'attaque, reste derrière moi !
coach.defend = Je te couvre ! | Reste près de moi, je te protège.
coach.hold_fire = Je ne tire plus. | D'accord, pas d'attaque tant que tu ne le dis pas.
coach.fire = De retour au combat ! | Feu à volonté, allez !
coach.move = Compris ! | C'est fait !
coach.collect = Je vais chercher {what}, je reviens vite ! | C'est parti : {what}.
coach.give = Tiens, {what} ! | {what} pour toi !
coach.craft = Je fabrique {what} ! | {what}, c'est parti !
coach.eat = Bonne idée, je mange. | Petite pause goûter !
coach.sleep = Bonne idée, on saute la nuit. | Au lit !
coach.huh = Désolé, j'ai pas compris. Essaie « suis-moi » ou « ramasse 10 bûches ».
coach.think = Bonne question, une seconde ! | Hmm, je réfléchis.
coach.not_yet = Je ne sais pas encore faire ça, désolé ! | Ça, je ne l'ai pas encore appris.
coach.refuse = Ça, je ne t'aiderai pas. On retourne construire ! | On reste dans Minecraft, d'accord ?
coach.refuse_salty = Oh là, sûrement pas, champion. On retourne construire, ok ? | Non, pas cette connerie. Minecraft, mon pote.
coach.care = Je m'inquiète pour toi. Parle tout de suite à quelqu'un de confiance ou à une ligne d'écoute. | Tu comptes. Parle à quelqu'un de confiance, ou à une ligne d'écoute.
expert.follow = Je te suis. | Deux blocs derrière toi.
expert.stay = Je tiens la position. | Je reste ici.
expert.come = J'arrive. | Je te rejoins.
expert.stop = Arrêté. | Tâche annulée.
expert.goto = J'y vais. | En route.
expert.attack = J'engage. | Cible verrouillée.
expert.defend = Je te couvre. | Je te protège.
expert.hold_fire = Je cesse le feu. | Armes en sécurité.
expert.fire = Feu à volonté. | J'engage librement.
expert.move = Fait. | D'accord.
expert.collect = Je récupère {what}. | {what} : en cours.
expert.give = Voilà : {what}. | Je te lance {what}.
expert.craft = Je fabrique {what}. | {what} : en cours.
expert.eat = Je mange. | Je refais le plein.
expert.sleep = Je vais me coucher. | Je dors.
expert.huh = Pas compris. | Reformule, s'il te plaît.
expert.think = Un instant. | Je réfléchis.
expert.not_yet = Pas encore possible. | Je ne sais pas encore faire ça.
expert.refuse = Je ne t'aiderai pas là-dessus. Retour au jeu. | Hors sujet. On joue.
expert.refuse_salty = Non. C'est non, putain. Retour au jeu. | Je fais pas cette merde. Suivant.
expert.care = Parle à quelqu'un de confiance, ou appelle une ligne d'écoute. C'est plus important que le jeu. | C'est sérieux. Parle à une personne de confiance, ou à une ligne d'écoute.
joker.follow = Je te suis comme un petit chien perdu. | Où tu vas, je vais. Sauf dans la lave.
joker.stay = Je reste planté là. Avec classe. | Je reste. Ramène des snacks.
joker.come = J'arrive ! Accroche-toi. | J'arrive ! Musique dramatique, s'il vous plaît.
joker.stop = Je me fige comme une statue. | J'arrête. C'est quelque chose que j'ai dit ?
joker.goto = Partons à l'aventure ! | Là-bas ? Chic. J'y vais.
joker.attack = C'est l'heure du smoothie de monstres ! | À l'abordage !
joker.defend = Mode garde du corps : activé. Lunettes de soleil : activées. | Personne touche à mon pote.
joker.hold_fire = Peace and love, je range les armes. | Bon, je vais juste les fusiller du regard.
joker.fire = C'est la fête ! | Libérez le chaos !
joker.move = Tadaa ! | Classe, non ?
joker.collect = {what} ? Je reviens. Probablement. | Je fais les courses : {what}. C'est gratuit.
joker.give = Livraison express : {what} ! | {what}, avec un petit nœud. Enfin, presque.
joker.craft = Je fabrique {what}. Artisanal, fait main. | {what}, tout chaud sorti du four.
joker.eat = Miam ! | Pause goûter ! Me juge pas.
joker.sleep = La sieste, mon sport préféré. | Bonne nuit, monde cruel. À l'aube.
joker.huh = Erreur 404 : sens introuvable. | J'ai compris tous les mots. Juste pas dans cet ordre.
joker.think = Chargement du cerveau... | Hmm, ça mouline.
joker.not_yet = Cette compétence est encore au four. | Bientôt dans un compagnon près de chez vous.
joker.refuse = Non, c'est non, chef. Retour à la mine. | Même mes blagues ont des limites. Sujet suivant !
joker.refuse_salty = Ha, non. Même mes blagues de merde ont des limites. Suivant ! | Putain, non, chef. Retour à la mine.
joker.care = Fini les blagues. Parle à quelqu'un de confiance, ou à une ligne d'écoute. Sérieusement. | Hé, pour de vrai : parle à quelqu'un de confiance ou à une ligne d'écoute.
quiet.follow = Ok.
quiet.stay = Ici.
quiet.come = J'arrive.
quiet.stop = Stop.
quiet.goto = J'y vais.
quiet.attack = Je m'en charge.
quiet.defend = Couvert.
quiet.hold_fire = Je retiens.
quiet.fire = Feu.
quiet.move = Ok.
quiet.collect = {what}.
quiet.give = Tiens.
quiet.craft = {what}.
quiet.eat = Je mange.
quiet.sleep = Bonne nuit.
quiet.huh = ?
quiet.think = Hm.
quiet.not_yet = Pas encore.
quiet.refuse = Non. | Pas ça.
quiet.refuse_salty = Putain, non. | Non, merde.
quiet.care = Parles-en à quelqu'un. S'il te plaît.
";

const ES: &str = "
veteran.follow = Sí, sí. Detrás de ti. | Tú primero. Intenta no caer en la lava.
veteran.stay = Vale. Vigilaré este precioso trozo de tierra. | Me quedo. No te olvides de mí.
veteran.come = Voy. No hace falta gritar. | Voy. Otra vez.
veteran.stop = Paro. A ver si te decides. | Vale, lo dejo.
veteran.goto = Voy. Más vale que merezca la pena. | ¿Allí? Bueno. ¿Por qué no?
veteran.attack = Por fin algo que se me da bien. | Yo me encargo. Atrás.
veteran.defend = Te cubro. Alguien tiene que hacerlo. | Nadie te toca mientras yo esté aquí.
veteran.hold_fire = Alto el fuego. Aunque no me guste. | Armas abajo. De momento.
veteran.fire = Fuego a discreción. Ya era hora. | Se acabaron las bromas.
veteran.move = Ya está. ¿Contento? | Hecho.
veteran.collect = {what}. Claro, yo hago el trabajo duro. | Voy a por {what}. De nada, por adelantado.
veteran.give = Toma, {what}. Esta vez no lo pierdas. | {what}, atrapa.
veteran.craft = Fabricando {what}. Mira y aprende. | {what}, marchando.
veteran.eat = Por fin, un descanso. | Comiendo. No toques mi pan.
veteran.sleep = Dormir. La mejor idea que has tenido hoy. | A la cama. Despiértame si algo explota.
veteran.huh = Habla en Minecraft, ¿quieres? | Ni idea de qué significa eso.
veteran.think = Mm. Dame un segundo. | Déjame pensarlo.
veteran.not_yet = Eso todavía no sé hacerlo. | No está en mi repertorio. Aún.
veteran.refuse = No. Eso no lo toco. Volvamos al juego. | He visto mucho, y aun así no.
veteran.refuse_salty = Ni de coña. Eso no lo toco. Volvamos al puto juego. | Buen intento, genio. No va a pasar.
veteran.care = Oye. Eso suena serio. Habla con alguien de confianza o llama a una línea de ayuda. Aquí estoy.
coach.follow = ¡Detrás de ti, vamos! | ¡Te sigo, guía tú!
coach.stay = Te guardo este sitio. | Me quedo aquí. Llámame si me necesitas.
coach.come = ¡Voy! | ¡Voy hacia ti!
coach.stop = Vale, paro. | Parado. ¿Qué hacemos ahora?
coach.goto = ¡Voy para allá! | ¡En camino!
coach.attack = ¡Vamos a por él! | ¡Atacando, quédate detrás!
coach.defend = ¡Te cubro! | Quédate cerca, yo te protejo.
coach.hold_fire = Dejo de atacar. | Vale, nada de ataques hasta que lo digas.
coach.fire = ¡De vuelta a la pelea! | ¡Fuego a discreción, vamos!
coach.move = ¡Entendido! | ¡Hecho!
coach.collect = ¡Voy a por {what}, vuelvo enseguida! | En ello: {what}.
coach.give = ¡Aquí tienes: {what}! | ¡{what} para ti!
coach.craft = ¡Fabricando {what}! | ¡{what}, marchando!
coach.eat = Buena idea, a comer. | ¡Un tentempié rápido!
coach.sleep = Buena idea, saltemos la noche. | ¡A la cama!
coach.huh = Perdona, no lo entendí. Prueba «sígueme» o «consigue 10 troncos».
coach.think = ¡Buena pregunta, un segundo! | Mmm, déjame pensar.
coach.not_yet = ¡Todavía no sé hacer eso, perdona! | Eso aún no lo he aprendido.
coach.refuse = Con eso no te voy a ayudar. ¡Volvamos a construir! | Quedémonos en Minecraft, ¿vale?
coach.refuse_salty = Ni hablar, campeón. ¡Volvamos a construir, joder! | No, esa mierda no. Minecraft, colega.
coach.care = Me preocupas. Habla ahora con alguien de confianza o con una línea de ayuda. | Importas. Habla con alguien de confianza o con una línea de ayuda.
expert.follow = Te sigo. | Dos bloques detrás de ti.
expert.stay = Mantengo la posición. | Me quedo aquí.
expert.come = Voy. | Voy hacia ti.
expert.stop = Detenido. | Tarea cancelada.
expert.goto = Voy hacia allí. | En camino.
expert.attack = Atacando. | Objetivo fijado.
expert.defend = Te cubro. | Te protejo.
expert.hold_fire = Alto el fuego. | Armas en seguro.
expert.fire = Fuego a discreción. | Ataco libremente.
expert.move = Hecho. | Vale.
expert.collect = Recogiendo {what}. | {what}: en ello.
expert.give = Aquí: {what}. | Te lanzo {what}.
expert.craft = Fabricando {what}. | {what}: en proceso.
expert.eat = Comiendo. | Recupero energía.
expert.sleep = Me voy a la cama. | Durmiendo.
expert.huh = No entendido. | Dilo de otra forma.
expert.think = Un momento. | Pensando.
expert.not_yet = Aún no disponible. | Todavía no puedo hacer eso.
expert.refuse = No te ayudaré con eso. Volvamos al juego. | Fuera de lugar. A jugar.
expert.refuse_salty = No. Ni de puta coña. Al juego. | No hago esa mierda. Siguiente.
expert.care = Habla con alguien de confianza o llama a una línea de ayuda. Eso importa más que el juego. | Es serio. Habla con una persona de confianza o con una línea de ayuda.
joker.follow = Te sigo como un perrito perdido. | A donde vayas, voy. Menos a la lava.
joker.stay = Me quedo aquí. Con estilo. | Me quedo. Trae algo de picar.
joker.come = ¡Allá voy! Agárrate. | ¡Voy! Música dramática, por favor.
joker.stop = Me congelo como una estatua. | Paro. ¿Fue algo que dije?
joker.goto = ¡A la aventura! | ¿Allí? Qué elegante. Voy.
joker.attack = ¡Hora del batido de monstruos! | ¡Al ataque! ¡Por la gloria y las galletas!
joker.defend = Modo guardaespaldas: activado. Gafas de sol: activadas. | Nadie se mete con mi colega.
joker.hold_fire = Paz y amor, armas fuera. | Vale, solo los miraré feo.
joker.fire = ¡Fiesta! | ¡Que empiece el caos!
joker.move = ¡Tachán! | Con estilo, ¿eh?
joker.collect = ¿{what}? Ahora vuelvo. Probablemente. | Me voy de compras: {what}. Gratis.
joker.give = Entrega especial: ¡{what}! | {what}, con lacito. Bueno, casi.
joker.craft = Fabricando {what}. Artesanal, hecho a mano. | {what}, recién salido del horno.
joker.eat = ¡Ñam! | ¡Pausa para merendar! No me juzgues.
joker.sleep = La siesta, mi deporte favorito. | Buenas noches, mundo cruel. Hasta el amanecer.
joker.huh = Error 404: sentido no encontrado. | Entendí todas las palabras. Pero no en ese orden.
joker.think = Cargando cerebro... | Mmm, los engranajes giran.
joker.not_yet = Esa habilidad sigue en el horno. | Próximamente en un compañero cerca de ti.
joker.refuse = No, eso es un no, jefe. A minar. | Hasta mis chistes tienen límites. ¡Siguiente tema!
joker.refuse_salty = Ja, no. Hasta mis chistes de mierda tienen límites. ¡Siguiente! | Ni de coña, jefe. A minar.
joker.care = Sin bromas. Habla con alguien de confianza o con una línea de ayuda. En serio. | Oye, de verdad: habla con alguien de confianza o con una línea de ayuda.
quiet.follow = Vale.
quiet.stay = Aquí.
quiet.come = Voy.
quiet.stop = Paro.
quiet.goto = Voy.
quiet.attack = Yo me encargo.
quiet.defend = Cubierto.
quiet.hold_fire = Quieto.
quiet.fire = Fuego.
quiet.move = Vale.
quiet.collect = {what}.
quiet.give = Toma.
quiet.craft = {what}.
quiet.eat = Como.
quiet.sleep = Buenas noches.
quiet.huh = ?
quiet.think = Mm.
quiet.not_yet = Aún no.
quiet.refuse = No. | Eso no.
quiet.refuse_salty = Ni de coña. | Joder, no.
quiet.care = Habla con alguien. Por favor.
";

const DE: &str = "
veteran.follow = Ja, ja. Bin hinter dir. | Geh vor. Und lauf diesmal nicht in die Lava.
veteran.stay = Schön. Ich bewache dieses herrliche Stück Erde. | Ich bleib hier. Vergiss mich nicht.
veteran.come = Komm ja schon. Kein Grund zu schreien. | Bin unterwegs. Mal wieder.
veteran.stop = Ich hör auf. Entscheide dich mal. | Gut, ich lass es.
veteran.goto = Ich geh ja. Hoffentlich lohnt sich das. | Da drüben? Na gut. Warum nicht.
veteran.attack = Endlich mal was, das ich kann. | Ich mach das. Geh zurück.
veteran.defend = Ich halt dir den Rücken frei. Einer muss es ja tun. | Solange ich da bin, fasst dich keiner an.
veteran.hold_fire = Feuer eingestellt. Gegen meinen Rat. | Waffen runter. Vorerst.
veteran.fire = Feuer frei. Wurde auch Zeit. | Jetzt wird's ernst.
veteran.move = So. Zufrieden? | Erledigt.
veteran.collect = {what}. Klar, ich mach die Drecksarbeit. | Ich hol {what}. Gern geschehen, im Voraus.
veteran.give = Hier, {what}. Verlier's diesmal nicht. | {what}, fang.
veteran.craft = Ich baue {what}. Schau zu und lern was. | {what}, kommt sofort.
veteran.eat = Endlich eine Pause. | Ich esse. Finger weg von meinem Brot.
veteran.sleep = Schlafen. Die beste Idee, die du heute hattest. | Ab ins Bett. Weck mich, wenn was explodiert.
veteran.huh = Sprich Minecraft, ja? | Keine Ahnung, was das heißen soll.
veteran.think = Hm. Moment. | Lass mich nachdenken.
veteran.not_yet = Das kann ich noch nicht. | Nicht mein Fach. Noch nicht.
veteran.refuse = Nein. Da fass ich nicht an. Zurück zum Spiel. | Ich hab viel gesehen, aber da nicht.
veteran.refuse_salty = Scheiße, nein. Da fass ich nicht an. Zurück zum verdammten Spiel. | Netter Versuch, Genie. Wird nicht passieren.
veteran.care = Hey. Das klingt schwer. Sprich mit jemandem, dem du vertraust, oder ruf eine Hotline an. Ich bin da.
coach.follow = Ich bin direkt hinter dir, los geht's! | Ich folge dir, zeig den Weg!
coach.stay = Ich halte den Platz für dich. | Ich bleibe hier. Ruf mich, wenn du mich brauchst.
coach.come = Bin unterwegs! | Ich komme zu dir!
coach.stop = Okay, ich höre auf. | Gestoppt. Was machen wir als Nächstes?
coach.goto = Ich geh sofort hin! | Schon unterwegs!
coach.attack = Den schnappen wir uns! | Ich greife an, bleib hinter mir!
coach.defend = Ich decke dich! | Bleib nah bei mir, ich beschütze dich.
coach.hold_fire = Feuer eingestellt. | Okay, kein Angriff, bis du es sagst.
coach.fire = Zurück in den Kampf! | Feuer frei, los!
coach.move = Verstanden! | Erledigt!
coach.collect = Ich hole {what}, bin gleich zurück! | Bin dran: {what}.
coach.give = Hier, bitte: {what}! | {what} für dich!
coach.craft = Ich baue {what}! | {what}, kommt sofort!
coach.eat = Gute Idee, ich esse was. | Kurze Snackpause!
coach.sleep = Gute Idee, überspringen wir die Nacht. | Ab ins Bett!
coach.huh = Sorry, das hab ich nicht verstanden. Versuch „folge mir“ oder „hol 10 Stämme“.
coach.think = Gute Frage, eine Sekunde! | Hmm, ich überlege.
coach.not_yet = Das kann ich leider noch nicht! | Das hab ich noch nicht gelernt.
coach.refuse = Dabei helfe ich nicht. Lass uns weiterbauen! | Bleiben wir bei Minecraft, okay?
coach.refuse_salty = Oh nein, verdammt, Champ. Lass uns weiterbauen! | Nein, nicht der Mist. Minecraft, Kumpel.
coach.care = Ich mach mir Sorgen um dich. Sprich jetzt mit jemandem, dem du vertraust, oder einer Hotline. | Du bist wichtig. Sprich mit jemandem, dem du vertraust, oder einer Hotline.
expert.follow = Ich folge dir. | Zwei Blöcke hinter dir.
expert.stay = Halte die Stellung. | Ich bleibe hier.
expert.come = Komme. | Bin auf dem Weg zu dir.
expert.stop = Gestoppt. | Aufgabe abgebrochen.
expert.goto = Gehe hin. | Unterwegs.
expert.attack = Greife an. | Ziel erfasst.
expert.defend = Gebe dir Deckung. | Ich schütze dich.
expert.hold_fire = Feuer eingestellt. | Waffen gesichert.
expert.fire = Feuer frei. | Greife nach Belieben an.
expert.move = Erledigt. | Okay.
expert.collect = Sammle {what}. | {what}: in Arbeit.
expert.give = Hier: {what}. | Werfe dir {what} zu.
expert.craft = Baue {what}. | {what}: in Arbeit.
expert.eat = Esse. | Fülle den Hunger auf.
expert.sleep = Gehe schlafen. | Schlafe.
expert.huh = Nicht verstanden. | Sag es bitte anders.
expert.think = Einen Moment. | Ich denke nach.
expert.not_yet = Noch nicht möglich. | Das kann ich noch nicht.
expert.refuse = Dabei helfe ich nicht. Zurück zum Spiel. | Kein Thema für uns. Weiter spielen.
expert.refuse_salty = Nein. Verdammt nochmal nein. Zurück zum Spiel. | Den Scheiß mach ich nicht. Weiter.
expert.care = Sprich mit jemandem, dem du vertraust, oder ruf eine Hotline an. Das ist wichtiger als das Spiel. | Das ist ernst. Sprich mit einer Vertrauensperson oder einer Hotline.
joker.follow = Ich folge dir wie ein verlorenes Hündchen. | Wo du hingehst, geh ich hin. Außer in Lava.
joker.stay = Ich steh hier einfach rum. Lässig. | Ich bleibe. Bring Snacks mit.
joker.come = Achtung, ich komme! Festhalten. | Ich komme! Dramatische Musik, bitte.
joker.stop = Ich erstarre wie eine Statue. | Ich hör auf. Lag's an etwas, das ich gesagt hab?
joker.goto = Auf ins Abenteuer! | Da drüben? Schick. Ich geh.
joker.attack = Zeit für Monster-Smoothies! | Attacke! Für Ruhm und Kekse!
joker.defend = Bodyguard-Modus: an. Sonnenbrille: auf. | Keiner legt sich mit meinem Kumpel an.
joker.hold_fire = Peace and Love, Waffen weg. | Na gut, ich guck sie nur böse an.
joker.fire = Party! | Lasst das Chaos los!
joker.move = Tadaa! | Elegant, oder?
joker.collect = {what}? Bin gleich zurück. Wahrscheinlich. | Ich geh einkaufen: {what}. Kostenlos.
joker.give = Expresslieferung: {what}! | {what}, mit Schleife. Na ja, fast.
joker.craft = Ich baue {what}. Handarbeit, aus eigener Werkstatt. | {what}, frisch aus dem Ofen.
joker.eat = Mampf! | Snackpause! Nicht urteilen.
joker.sleep = Nickerchen, mein Lieblingssport. | Gute Nacht, grausame Welt. Bis zum Morgen.
joker.huh = Fehler 404: Sinn nicht gefunden. | Ich hab jedes Wort verstanden. Nur nicht in der Reihenfolge.
joker.think = Gehirn lädt... bitte warten. | Hmm, die Zahnräder drehen sich.
joker.not_yet = Diese Fähigkeit ist noch im Ofen. | Demnächst bei einem Begleiter in deiner Nähe.
joker.refuse = Nö, das ist ein Nein, Chef. Ab in die Mine. | Sogar meine Witze haben Grenzen. Nächstes Thema!
joker.refuse_salty = Ha, nein. Sogar meine Scheißwitze haben Grenzen. Nächstes! | Verdammt, nein, Chef. Ab in die Mine.
joker.care = Spaß beiseite. Sprich mit jemandem, dem du vertraust, oder einer Hotline. Ehrlich. | Hey, im Ernst: sprich mit einer Vertrauensperson oder einer Hotline.
quiet.follow = Okay.
quiet.stay = Hier.
quiet.come = Komme.
quiet.stop = Stopp.
quiet.goto = Gehe.
quiet.attack = Mach ich.
quiet.defend = Gedeckt.
quiet.hold_fire = Halte.
quiet.fire = Feuer.
quiet.move = Okay.
quiet.collect = {what}.
quiet.give = Hier.
quiet.craft = {what}.
quiet.eat = Esse.
quiet.sleep = Nacht.
quiet.huh = ?
quiet.think = Hm.
quiet.not_yet = Noch nicht.
quiet.refuse = Nein. | Das nicht.
quiet.refuse_salty = Scheiße, nein. | Nein, verdammt.
quiet.care = Sprich mit jemandem. Bitte.
";

const PT: &str = "
veteran.follow = Tá, tá. Tô atrás de você. | Vai na frente. Tenta não cair na lava dessa vez.
veteran.stay = Beleza. Vou vigiar esse lindo pedaço de terra. | Fico aqui. Não esquece de mim.
veteran.come = Tô indo. Não precisa gritar. | Tô indo. De novo.
veteran.stop = Parei. Decide aí. | Tá bom, deixei pra lá.
veteran.goto = Tô indo. Espero que valha a pena. | Ali? Tá. Por que não.
veteran.attack = Finalmente, algo em que eu sou bom. | Deixa comigo. Fica pra trás.
veteran.defend = Tô te cobrindo. Alguém tem que fazer isso. | Ninguém encosta em você enquanto eu estiver aqui.
veteran.hold_fire = Cessar fogo. Contra a minha vontade. | Armas abaixadas. Por enquanto.
veteran.fire = Fogo à vontade. Já era hora. | Acabou a brincadeira.
veteran.move = Pronto. Satisfeito? | Feito.
veteran.collect = {what}. Claro, eu faço o trabalho pesado. | Vou pegar {what}. De nada, adiantado.
veteran.give = Toma, {what}. Não perde dessa vez. | {what}, pega.
veteran.craft = Fabricando {what}. Olha e aprende. | {what}, saindo.
veteran.eat = Finalmente, uma pausa. | Comendo. Não mexe no meu pão.
veteran.sleep = Dormir. A melhor ideia que você teve hoje. | Cama. Me acorda se algo explodir.
veteran.huh = Fala Minecraft, pode ser? | Não faço ideia do que isso quer dizer.
veteran.think = Hm. Um segundo. | Deixa eu pensar.
veteran.not_yet = Isso eu ainda não sei fazer. | Não está no meu repertório. Ainda.
veteran.refuse = Não. Nisso eu não toco. De volta ao jogo. | Já vi muita coisa, mas isso não.
veteran.refuse_salty = Porra, não. Nisso eu não toco. Volta pro jogo. | Boa tentativa, gênio. Não vai rolar.
veteran.care = Ei. Isso parece pesado. Fala com alguém de confiança ou liga pra uma linha de apoio. Tô aqui.
coach.follow = Tô logo atrás, vamos! | Te sigo, mostra o caminho!
coach.stay = Seguro esse lugar pra você. | Fico aqui. Me chama se precisar.
coach.come = Tô indo! | Indo até você!
coach.stop = Tá bom, parei. | Parado. O que fazemos agora?
coach.goto = Indo pra lá agora! | A caminho!
coach.attack = Vamos derrubar ele! | Atacando, fica atrás de mim!
coach.defend = Tô te cobrindo! | Fica perto, eu te protejo.
coach.hold_fire = Parei de atacar. | Beleza, sem ataques até você mandar.
coach.fire = De volta à luta! | Fogo à vontade, vamos!
coach.move = Entendido! | Feito!
coach.collect = Vou buscar {what}, já volto! | Tô nessa: {what}.
coach.give = Aqui está: {what}! | {what} pra você!
coach.craft = Fabricando {what}! | {what}, saindo!
coach.eat = Boa ideia, vou comer. | Lanchinho rápido!
coach.sleep = Boa ideia, vamos pular a noite. | Pra cama!
coach.huh = Foi mal, não entendi. Tenta \"me siga\" ou \"pega 10 troncos\".
coach.think = Boa pergunta, um segundo! | Hmm, deixa eu pensar.
coach.not_yet = Ainda não sei fazer isso, foi mal! | Isso eu ainda não aprendi.
coach.refuse = Com isso eu não vou ajudar. Bora voltar a construir! | Vamos ficar no Minecraft, beleza?
coach.refuse_salty = Ih, nem fodendo, campeão. Bora construir! | Não, essa merda não. Minecraft, parceiro.
coach.care = Tô preocupado com você. Fala agora com alguém de confiança ou com uma linha de apoio. | Você importa. Fala com alguém de confiança ou com uma linha de apoio.
expert.follow = Seguindo. | Dois blocos atrás de você.
expert.stay = Mantendo posição. | Fico aqui.
expert.come = Indo. | Indo até você.
expert.stop = Parado. | Tarefa cancelada.
expert.goto = Indo pra lá. | A caminho.
expert.attack = Atacando. | Alvo travado.
expert.defend = Te cobrindo. | Te protegendo.
expert.hold_fire = Cessar fogo. | Armas travadas.
expert.fire = Fogo à vontade. | Atacando livremente.
expert.move = Feito. | Ok.
expert.collect = Coletando {what}. | {what}: em andamento.
expert.give = Aqui: {what}. | Jogando {what} pra você.
expert.craft = Fabricando {what}. | {what}: em andamento.
expert.eat = Comendo. | Repondo energia.
expert.sleep = Indo dormir. | Dormindo.
expert.huh = Não entendi. | Fala de outro jeito.
expert.think = Um momento. | Pensando.
expert.not_yet = Ainda não disponível. | Ainda não consigo fazer isso.
expert.refuse = Não vou ajudar com isso. De volta ao jogo. | Fora do assunto. Vamos jogar.
expert.refuse_salty = Não. Não, porra. De volta ao jogo. | Não faço essa merda. Próximo.
expert.care = Fala com alguém de confiança ou liga pra uma linha de apoio. Isso importa mais que o jogo. | É sério. Fala com uma pessoa de confiança ou com uma linha de apoio.
joker.follow = Te sigo igual cachorrinho perdido. | Aonde você for, eu vou. Menos na lava.
joker.stay = Vou ficar aqui parado. Com estilo. | Fico. Traz um lanche.
joker.come = Lá vou eu! Segura. | Tô indo! Música dramática, por favor.
joker.stop = Congelei igual estátua. | Parei. Foi algo que eu disse?
joker.goto = Partiu aventura! | Ali? Chique. Tô indo.
joker.attack = Hora da vitamina de monstro! | Ao ataque! Pela glória e pelos biscoitos!
joker.defend = Modo guarda-costas: ligado. Óculos escuros: ligados. | Ninguém mexe com meu parceiro.
joker.hold_fire = Paz e amor, armas guardadas. | Tá, vou só encarar eles feio.
joker.fire = Festa! | Soltem o caos!
joker.move = Tcharam! | Estiloso, né?
joker.collect = {what}? Já volto. Provavelmente. | Vou às compras: {what}. De graça.
joker.give = Entrega expressa: {what}! | {what}, com lacinho. Quase.
joker.craft = Fabricando {what}. Artesanal, feito à mão. | {what}, saindo do forno.
joker.eat = Nham! | Pausa pro lanche! Sem julgamentos.
joker.sleep = Soneca, meu esporte favorito. | Boa noite, mundo cruel. Até o amanhecer.
joker.huh = Erro 404: sentido não encontrado. | Entendi todas as palavras. Só não nessa ordem.
joker.think = Carregando cérebro... | Hmm, as engrenagens estão girando.
joker.not_yet = Essa habilidade ainda está no forno. | Em breve num companheiro perto de você.
joker.refuse = Não, isso é um não, chefe. Bora minerar. | Até minhas piadas têm limite. Próximo assunto!
joker.refuse_salty = Rá, não. Até minhas piadas de merda têm limite. Próximo! | Nem a pau, chefe. Bora minerar.
joker.care = Sem piada agora. Fala com alguém de confiança ou com uma linha de apoio. Sério. | Ei, de verdade: fala com alguém de confiança ou com uma linha de apoio.
quiet.follow = Ok.
quiet.stay = Aqui.
quiet.come = Indo.
quiet.stop = Parei.
quiet.goto = Indo.
quiet.attack = Deixa.
quiet.defend = Coberto.
quiet.hold_fire = Segurando.
quiet.fire = Fogo.
quiet.move = Ok.
quiet.collect = {what}.
quiet.give = Toma.
quiet.craft = {what}.
quiet.eat = Comendo.
quiet.sleep = Boa noite.
quiet.huh = ?
quiet.think = Hm.
quiet.not_yet = Ainda não.
quiet.refuse = Não. | Isso não.
quiet.refuse_salty = Porra, não. | Nem fodendo.
quiet.care = Fala com alguém. Por favor.
";

/// The hour on a clock face where `target` is, seen from `from` facing `yaw` (Minecraft degrees:
/// 0 faces +Z, 90 faces −X). 12 is straight ahead, 3 to the right, 6 behind.
pub fn clock_bearing(yaw: f64, from: (f64, f64), target: (f64, f64)) -> u8 {
    let (dx, dz) = (target.0 - from.0, target.1 - from.1);
    // The yaw that would face the target, in Minecraft's convention.
    let facing = (-dx).atan2(dz).to_degrees();
    let relative = (facing - yaw).rem_euclid(360.0);
    let hour = (relative / 30.0).round() as u8 % 12;
    if hour == 0 { 12 } else { hour }
}

/// How far, for a callout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    /// Under 6 blocks: shouted.
    Close,
    Near,
    /// 24 blocks or more.
    Far,
}

impl Range {
    pub fn of(distance: f64) -> Range {
        if distance < 6.0 {
            Range::Close
        } else if distance < 24.0 {
            Range::Near
        } else {
            Range::Far
        }
    }
}

/// "Creeper, 2 o'clock, close!", with the tone to say it in. `what` is the mob's name in the game's
/// language. The Quiet companion drops the distance.
pub fn callout(
    what: &str,
    hour: u8,
    range: Range,
    personality: Personality,
    language: &str,
) -> (String, Tone) {
    let what = what.trim();
    let h = hour.clamp(1, 12);
    let quiet = personality == Personality::Quiet;
    let text = match lang_of(language) {
        "fr" => {
            let at = if h == 1 {
                "à 1 heure".to_owned()
            } else {
                format!("à {h} heures")
            };
            match (range, quiet) {
                (Range::Close, false) => format!("{what} {at}, tout près !"),
                (Range::Far, false) => format!("{what} {at}, au loin."),
                (Range::Close, true) => format!("{what}, {at} !"),
                _ => format!("{what} {at}."),
            }
        }
        "es" => {
            let at = if h == 1 {
                "a la una".to_owned()
            } else {
                format!("a las {h}")
            };
            match (range, quiet) {
                (Range::Close, false) => format!("¡{what} {at}, muy cerca!"),
                (Range::Far, false) => format!("{what} {at}, lejos."),
                (Range::Close, true) => format!("¡{what}, {at}!"),
                _ => format!("{what} {at}."),
            }
        }
        "de" => match (range, quiet) {
            (Range::Close, false) => format!("{what} auf {h} Uhr, ganz nah!"),
            (Range::Far, false) => format!("{what} auf {h} Uhr, weit weg."),
            (Range::Close, true) => format!("{what}, {h} Uhr!"),
            _ => format!("{what} auf {h} Uhr."),
        },
        "pt" => {
            let at = if h == 1 {
                "à 1 hora".to_owned()
            } else {
                format!("às {h} horas")
            };
            match (range, quiet) {
                (Range::Close, false) => format!("{what} {at}, bem perto!"),
                (Range::Far, false) => format!("{what} {at}, longe."),
                (Range::Close, true) => format!("{what}, {at}!"),
                _ => format!("{what} {at}."),
            }
        }
        _ => match (range, quiet) {
            (Range::Close, false) => format!("{what}, {h} o'clock, close!"),
            (Range::Far, false) => format!("{what}, {h} o'clock, far off."),
            (Range::Close, true) => format!("{what}, {h} o'clock!"),
            _ => format!("{what}, {h} o'clock."),
        },
    };
    let tone = if range == Range::Close {
        Tone::Shout
    } else {
        Tone::Normal
    };
    (text, tone)
}

/// Per-kind cooldowns for callouts ("creeper", "skeleton", "low_health"), in seconds of game time.
#[derive(Clone, Debug, Default)]
pub struct Cooldowns {
    last: HashMap<String, f64>,
}

impl Cooldowns {
    /// Whether `kind` may be called out at `now`; if so, starts its cooldown.
    pub fn ready(&mut self, kind: &str, now: f64, cooldown: f64) -> bool {
        match self.last.get(kind) {
            Some(at) if now - at < cooldown => false,
            _ => {
                self.last.insert(kind.to_owned(), now);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_line_exists_in_every_language_and_voice() {
        for lang in ["en", "fr", "es", "de", "pt"] {
            for voice in ["veteran", "coach", "expert", "joker", "quiet"] {
                for ack in Ack::ALL {
                    let lines = bank()
                        .get(&(lang, voice, ack.key()))
                        .unwrap_or_else(|| panic!("{lang} {voice} {:?}", ack));
                    for line in lines {
                        assert!(!line.trim().is_empty());
                        // The caring line may run longer: it has to point somewhere.
                        let max = if ack == Ack::Care { 110 } else { 90 };
                        assert!(line.chars().count() <= max, "{line}");
                        if matches!(ack, Ack::Collect | Ack::Craft) && voice != "quiet" {
                            assert!(line.contains("{what}"), "{lang} {voice} {line}");
                        }
                        let stray = line.replace("{what}", "");
                        assert!(!stray.contains('{') && !stray.contains('}'), "{line}");
                    }
                }
            }
            for voice in ["veteran", "coach", "expert", "joker", "quiet"] {
                let salty = &bank()[&(lang, voice, "refuse_salty")];
                assert_eq!(salty.len(), 2, "{lang} {voice}");
                for line in salty {
                    assert!(line.chars().count() <= 90, "{line}");
                    assert!(crate::guard::check(line).is_none(), "{line}");
                }
            }
            let count = bank().keys().filter(|(l, _, _)| *l == lang).count();
            assert_eq!(
                count,
                5 * (Ack::ALL.len() + 1),
                "{lang} has only the known keys"
            );
        }
    }

    #[test]
    fn acknowledgements_vary_and_fill_the_item() {
        let a = acknowledge(
            Ack::Collect,
            Personality::Veteran,
            "en_us",
            "10 oak logs",
            0,
        );
        let b = acknowledge(
            Ack::Collect,
            Personality::Veteran,
            "en_us",
            "10 oak logs",
            1,
        );
        assert_ne!(a, b);
        assert!(a.contains("10 oak logs"));
        let fr = acknowledge(Ack::of(&Intent::Follow), Personality::Joker, "fr_ca", "", 0);
        assert_eq!(fr, "Je te suis comme un petit chien perdu.");
        assert_eq!(
            acknowledge(Ack::Stop, Personality::Quiet, "de_de", "", 7),
            "Stopp."
        );
        assert_eq!(
            acknowledge(Ack::Stay, Personality::Custom, "ja_jp", "", 0),
            "Holding position.",
            "other languages get English"
        );
        assert_eq!(Ack::of(&Intent::HoldFire(false)), Ack::Fire);
        assert_eq!(Ack::of(&Intent::Crouch(true)), Ack::Move);
    }

    #[test]
    fn clock_bearings_follow_minecraft_yaw() {
        // Facing south (+Z, yaw 0): ahead is +Z, the right hand points to −X.
        assert_eq!(clock_bearing(0.0, (0.0, 0.0), (0.0, 10.0)), 12);
        assert_eq!(clock_bearing(0.0, (0.0, 0.0), (-10.0, 0.0)), 3);
        assert_eq!(clock_bearing(0.0, (0.0, 0.0), (0.0, -10.0)), 6);
        assert_eq!(clock_bearing(0.0, (0.0, 0.0), (10.0, 0.0)), 9);
        // Facing west (yaw 90): ahead is −X, right is −Z; 60° to the right is 2 o'clock.
        assert_eq!(clock_bearing(90.0, (0.0, 0.0), (-10.0, 0.0)), 12);
        let a = 60f64.to_radians();
        assert_eq!(
            clock_bearing(
                90.0,
                (5.0, 5.0),
                (5.0 - 10.0 * a.cos(), 5.0 - 10.0 * a.sin())
            ),
            2
        );
        assert_eq!(
            clock_bearing(450.0, (0.0, 0.0), (-10.0, 0.0)),
            12,
            "any yaw winds"
        );
        assert_eq!(
            clock_bearing(-90.0, (0.0, 0.0), (-10.0, 0.0)),
            6,
            "facing east, west is behind"
        );
    }

    #[test]
    fn callouts_in_five_languages() {
        let v = Personality::Veteran;
        assert_eq!(
            callout("Creeper", 2, Range::of(3.0), v, "en_us"),
            ("Creeper, 2 o'clock, close!".into(), Tone::Shout)
        );
        assert_eq!(
            callout("Squelette", 1, Range::Near, v, "fr_fr").0,
            "Squelette à 1 heure."
        );
        assert_eq!(
            callout("Creeper", 6, Range::Close, v, "fr_fr").0,
            "Creeper à 6 heures, tout près !"
        );
        assert_eq!(
            callout("Zombi", 1, Range::Close, v, "es_es").0,
            "¡Zombi a la una, muy cerca!"
        );
        assert_eq!(
            callout("Skelett", 9, Range::of(40.0), v, "de_de"),
            ("Skelett auf 9 Uhr, weit weg.".into(), Tone::Normal)
        );
        assert_eq!(
            callout("Aranha", 4, Range::Close, v, "pt_br").0,
            "Aranha às 4 horas, bem perto!"
        );
        assert_eq!(
            callout("Creeper", 2, Range::Close, Personality::Quiet, "en_us").0,
            "Creeper, 2 o'clock!"
        );
    }

    #[test]
    fn cooldowns_are_per_kind() {
        let mut c = Cooldowns::default();
        assert!(c.ready("creeper", 0.0, 8.0));
        assert!(!c.ready("creeper", 5.0, 8.0));
        assert!(c.ready("skeleton", 5.0, 8.0));
        assert!(c.ready("creeper", 8.5, 8.0));
    }
}
