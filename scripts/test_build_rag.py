import unittest

from scripts.build_rag import generate_documents


class GenerateDocumentsTests(unittest.TestCase):
    def test_nested_object_lists_split_into_entry_documents(self):
        payload = {
            "projects": [
                {
                    "title": "Micro Mages",
                    "description": "Porting work.",
                    "tech": ["Python"],
                },
                {
                    "title": "ZQSDev Terminal",
                    "description": "Interactive resume terminal.",
                    "tech": ["Rust", "WebAssembly"],
                },
            ]
        }

        documents = list(generate_documents("projects", payload))

        self.assertEqual(len(documents), 2)
        self.assertEqual(documents[0][0], "projects-projects-micro-mages")
        self.assertEqual(documents[1][0], "projects-projects-zqsdev-terminal")
        self.assertEqual(documents[1][1], "projects: ZQSDev Terminal")
        self.assertIn("Label: ZQSDev Terminal", documents[1][2])
        self.assertIn('"WebAssembly"', documents[1][2])

    def test_scalar_lists_remain_grouped_by_section(self):
        payload = {"Cloud & DevOps": ["AWS", "Docker", "Kubernetes"]}

        documents = list(generate_documents("skills", payload))

        self.assertEqual(len(documents), 1)
        self.assertEqual(documents[0][0], "skills-cloud-devops")
        self.assertIn('"AWS"', documents[0][2])


if __name__ == "__main__":
    unittest.main()
