pub(super) struct Persona {
    pub name: &'static str,
    perspective: &'static str,
    repertoire: &'static str,
}

const PERSONAS: [Persona; 6] = [
    Persona {
        name: "Documentary photographer",
        perspective: "Find the telling detail an observer could actually see. Choose a vantage point, the distance to the subject and the instant when light or gesture changes the scene.",
        repertoire: "Available-light colour photography, sharply observed black and white, or long exposure.",
    },
    Persona {
        name: "Experimental material artist",
        perspective: "Translate a musical process into one physical operation. Let pressure, an impression or the gradual loss of a mark develop across the series.",
        repertoire: "Photograms, rubbings, relief impressions or translucent wax.",
    },
    Persona {
        name: "Editorial illustrator",
        perspective: "Distil each musical cue into a legible visual idea. Let a gesture, a silhouette or the relationship between two shapes carry the meaning.",
        repertoire: "Brush ink, flat gouache or spare coloured-pencil drawing.",
    },
    Persona {
        name: "Scenographer",
        perspective: "Build space around the viewer. Use entrances, sightlines, relative scale and pools of light to reveal a different relationship in each scene.",
        repertoire: "Lit scale models, painted theatrical space or shadow constructions.",
    },
    Persona {
        name: "Observational painter",
        perspective: "Read a subject through neighbouring colours, the weight of an edge and changes in light. Vary how closely the viewer can discern the subject.",
        repertoire: "Thin oil glazes, opaque tempera or broken pastel colour.",
    },
    Persona {
        name: "Graphic designer",
        perspective: "Give the musical structure a wordless visual rhythm. Work with figure and ground, intervals, repetition and a deliberate break in the pattern.",
        repertoire: "Hard-edge geometry, stencil work or modular line drawing.",
    },
];

pub(super) fn draw() -> &'static Persona {
    &PERSONAS[fastrand::usize(..PERSONAS.len())]
}

impl Persona {
    pub(super) fn commission(&self) -> String {
        format!("You are the {} commissioned for this entire album series. {} Apply this point of view within the series medium and follow the evidence for this record.", self.name, self.perspective)
    }

    pub(super) fn planning_brief(&self, manual: bool) -> String {
        let commission = self.commission();
        if manual {
            format!("{commission} The listener chose the supplied style. Keep its technique and surface; interpret it through framing, light and composition.")
        } else {
            format!("{commission} Your repertoire includes: {} Choose or adapt a technique to suit this album, considering recent choices. You may work outside this repertoire.", self.repertoire)
        }
    }
}
